//! Qwen3-ASR 音频塔：三次 stride-2 卷积 + 18 层窗口注意力 transformer + 投影。
//!
//! 与官方实现对齐的要点：
//! - mel 按 100 帧一个 chunk 独立过 conv（等价于官方 `view(batch, chunks, ...)`），
//!   因为 conv 的 `padding=1` 在每个 chunk 边界都会重新补零；
//! - 每个 chunk 加**同一张**正弦位置编码表（`positional_embedding[:13]`），
//!   不是跨 chunk 递增的位置——这是 Qwen3-ASR 与 Whisper 的一个显著差别；
//! - 注意力是窗口内双向的，窗口 104 token（= 13 × 8 个 chunk），窗口间互不可见。

use candle_core::{Device, Tensor};
use candle_nn::{
    conv2d, layer_norm, linear, linear_no_bias, ops::softmax_last_dim, Conv2d, Conv2dConfig,
    LayerNorm, Linear, Module, VarBuilder,
};

use super::config::{AudioTowerConfig, WeightLayout};
use crate::InferError;

pub struct Encoder {
    conv1: Conv2d,
    conv2: Conv2d,
    conv3: Conv2d,
    conv_out: Linear,
    layers: Vec<EncLayer>,
    ln_post: LayerNorm,
    proj1: Linear,
    proj2: Linear,
    /// `[tokens_per_chunk, d_model]`，每个 chunk 复用。
    pe: Tensor,
    cfg: AudioTowerConfig,
}

struct EncLayer {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    o_proj: Linear,
    attn_norm: LayerNorm,
    fc1: Linear,
    fc2: Linear,
    ffn_norm: LayerNorm,
}

impl Encoder {
    pub fn load(
        vb: &VarBuilder,
        cfg: &AudioTowerConfig,
        layout: WeightLayout,
        dev: &Device,
    ) -> Result<Self, InferError> {
        let vb_audio = vb.pp(layout.audio_tower());

        // 三个 stride-2 卷积把 128 个 mel bin 压到 16，通道固定 480。
        let conv_cfg = Conv2dConfig {
            stride: 2,
            padding: 1,
            ..Default::default()
        };
        let ch = cfg.downsample_hidden_size;
        let conv1 = conv2d(1, ch, 3, conv_cfg, vb_audio.pp("conv2d1"))?;
        let conv2 = conv2d(ch, ch, 3, conv_cfg, vb_audio.pp("conv2d2"))?;
        let conv3 = conv2d(ch, ch, 3, conv_cfg, vb_audio.pp("conv2d3"))?;

        // 频域经过三次卷积：128 → 64 → 32 → 16
        let freq_after = super::config::conv_out_len(super::config::conv_out_len(
            super::config::conv_out_len(cfg.num_mel_bins),
        ));
        let conv_out = linear_no_bias(ch * freq_after, cfg.d_model, vb_audio.pp("conv_out"))?;

        let hd = cfg.head_dim();
        let mut layers = Vec::with_capacity(cfg.layers);
        for i in 0..cfg.layers {
            let lp = vb_audio.pp(format!("layers.{i}"));
            layers.push(EncLayer {
                q_proj: linear(cfg.d_model, cfg.heads * hd, lp.pp("self_attn.q_proj"))?,
                k_proj: linear(cfg.d_model, cfg.heads * hd, lp.pp("self_attn.k_proj"))?,
                v_proj: linear(cfg.d_model, cfg.heads * hd, lp.pp("self_attn.v_proj"))?,
                o_proj: linear(cfg.heads * hd, cfg.d_model, lp.pp("self_attn.out_proj"))?,
                attn_norm: layer_norm(cfg.d_model, 1e-5, lp.pp("self_attn_layer_norm"))?,
                fc1: linear(cfg.d_model, cfg.ffn_dim, lp.pp("fc1"))?,
                fc2: linear(cfg.ffn_dim, cfg.d_model, lp.pp("fc2"))?,
                ffn_norm: layer_norm(cfg.d_model, 1e-5, lp.pp("final_layer_norm"))?,
            });
        }

        let ln_post = layer_norm(cfg.d_model, 1e-5, vb_audio.pp("ln_post"))?;
        let (p1, p2) = layout.projector();
        let proj1 = linear(cfg.d_model, cfg.d_model, vb.pp(p1))?;
        let proj2 = linear(cfg.d_model, cfg.output_dim, vb.pp(p2))?;

        let pe = sinusoidal_pe(cfg.tokens_per_chunk(), cfg.d_model, dev)?;

        Ok(Self {
            conv1,
            conv2,
            conv3,
            conv_out,
            layers,
            ln_post,
            proj1,
            proj2,
            pe,
            cfg: cfg.clone(),
        })
    }

    /// `mel`: `[mel_bins, frames]`，`frames` 必须是 chunk 的整数倍。
    /// `valid_tokens`: 只保留前多少个 token（对齐官方按 mask 截取）。
    pub fn forward(&self, mel: &Tensor, valid_tokens: usize) -> Result<Tensor, InferError> {
        let chunk = self.cfg.chunk_frames();
        let frames = mel.dims()[1];
        if !frames.is_multiple_of(chunk) {
            return Err(InferError::Config(format!(
                "mel 帧数 {frames} 不是 chunk {chunk} 的整数倍"
            )));
        }
        let n_chunks = frames / chunk;
        let dev = mel.device();

        let mut chunks = Vec::with_capacity(n_chunks);
        for c in 0..n_chunks {
            // [128, 100] → [1, 1, 128, 100]
            let x = mel
                .narrow(1, c * chunk, chunk)?
                .unsqueeze(0)?
                .unsqueeze(0)?;
            let x = self.conv1.forward(&x)?.gelu()?;
            let x = self.conv2.forward(&x)?.gelu()?;
            let x = self.conv3.forward(&x)?.gelu()?;

            let dims = x.dims();
            let (ch, freq, w) = (dims[1], dims[2], dims[3]);
            // [1, C, F, W] → [W, C*F]
            let x = x
                .permute([0, 3, 1, 2])?
                .contiguous()?
                .reshape((w, ch * freq))?;
            let x = self.conv_out.forward(&x)?;
            let pe = self.pe.to_dtype(x.dtype())?;
            chunks.push(x.broadcast_add(&pe)?);
        }

        let mut x = Tensor::cat(&chunks, 0)?;
        let total = x.dims()[0];

        let window = self.cfg.window_tokens();
        // [total, total]，在 heads 维上广播；dtype 跟随激活，f16 推理时不能混 f32
        let mask = window_mask(total, window, dev)?.to_dtype(x.dtype())?;

        for layer in &self.layers {
            x = layer.forward(&x, &mask, self.cfg.heads, self.cfg.head_dim())?;
        }

        let x = self.ln_post.forward(&x)?;
        let x = self.proj1.forward(&x)?.gelu()?;
        let x = self.proj2.forward(&x)?;

        let valid = valid_tokens.min(total);
        Ok(x.narrow(0, 0, valid)?)
    }
}

impl EncLayer {
    fn forward(
        &self,
        x: &Tensor,
        mask: &Tensor,
        heads: usize,
        head_dim: usize,
    ) -> Result<Tensor, InferError> {
        let seq = x.dims()[0];

        let xn = self.attn_norm.forward(x)?;
        let q = self.q_proj.forward(&xn)?;
        let k = self.k_proj.forward(&xn)?;
        let v = self.v_proj.forward(&xn)?;

        // [seq, d] → [heads, seq, head_dim]
        let q = q
            .reshape((seq, heads, head_dim))?
            .transpose(0, 1)?
            .contiguous()?;
        let k = k
            .reshape((seq, heads, head_dim))?
            .transpose(0, 1)?
            .contiguous()?;
        let v = v
            .reshape((seq, heads, head_dim))?
            .transpose(0, 1)?
            .contiguous()?;

        let scale = 1.0 / (head_dim as f64).sqrt();
        let scores = (q.matmul(&k.transpose(1, 2)?.contiguous()?)? * scale)?;
        let scores = scores.broadcast_add(mask)?;
        let probs = softmax_last_dim(&scores)?;
        let out = probs
            .matmul(&v)?
            .transpose(0, 1)?
            .contiguous()?
            .reshape((seq, heads * head_dim))?;
        let out = self.o_proj.forward(&out)?;

        let x = (x + out)?;
        let xn = self.ffn_norm.forward(&x)?;
        let h = self.fc1.forward(&xn)?.gelu()?;
        let h = self.fc2.forward(&h)?;
        Ok((&x + h)?)
    }
}

/// 窗口掩码：窗口内 0，窗口外 -inf。`[total, total]`。
fn window_mask(total: usize, window: usize, dev: &Device) -> Result<Tensor, InferError> {
    if window == 0 {
        return Err(InferError::Config("注意力窗口为 0".into()));
    }
    let mut data = vec![f32::NEG_INFINITY; total * total];
    for start in (0..total).step_by(window) {
        let end = (start + window).min(total);
        for i in start..end {
            for j in start..end {
                data[i * total + j] = 0.0;
            }
        }
    }
    Ok(Tensor::from_vec(data, (total, total), dev)?)
}

/// Qwen3-ASR 的正弦位置编码：`sin` 在前半、`cos` 在后半，**不交织**；
/// 时间尺度是 `ln(10000) / (half - 1)`（比标准 transformer 少一格）。
fn sinusoidal_pe(n_pos: usize, d_model: usize, dev: &Device) -> Result<Tensor, InferError> {
    let half = d_model / 2;
    if half < 2 {
        return Err(InferError::Config("d_model 太小".into()));
    }
    let log_timescale = 10_000f32.ln() / (half - 1) as f32;
    let inv: Vec<f32> = (0..half)
        .map(|d| (-(d as f32) * log_timescale).exp())
        .collect();

    let mut data = vec![0f32; n_pos * d_model];
    for p in 0..n_pos {
        for d in 0..half {
            let angle = p as f32 * inv[d];
            data[p * d_model + d] = angle.sin();
            data[p * d_model + half + d] = angle.cos();
        }
    }
    Ok(Tensor::from_vec(data, (n_pos, d_model), dev)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sinusoidal_shape_and_columns() {
        let dev = Device::Cpu;
        let pe = sinusoidal_pe(13, 896, &dev).unwrap();
        assert_eq!(pe.dims(), &[13, 896]);
        let v = pe.to_vec2::<f32>().unwrap();
        // 位置 0：sin(0)=0，cos(0)=1
        assert!(v[0][0].abs() < 1e-6);
        assert!((v[0][448] - 1.0).abs() < 1e-6);
        // 频率越高（d 越大），同一位置的 sin 越接近 0
        assert!(v[1][447].abs() < v[1][0].abs());
    }

    #[test]
    fn window_mask_blocks_across_windows() {
        let m = window_mask(130, 104, &Device::Cpu).unwrap();
        let v = m.to_vec2::<f32>().unwrap();
        assert_eq!(v[0][103], 0.0);
        assert_eq!(v[0][104], f32::NEG_INFINITY);
        assert_eq!(v[129][104], 0.0);
        assert_eq!(v[104][0], f32::NEG_INFINITY);
    }
}
