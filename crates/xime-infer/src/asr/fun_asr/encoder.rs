//! Fun-ASR-Nano 的音频编码器（SenseVoice 的 SAN-M 结构）与投影适配层。
//!
//! ```text
//! 560 维 LFR 特征
//!   × sqrt(512) + 正弦位置编码（位置从 1 开始）
//!   → 70 层 SAN-M：
//!        h = LayerNorm(x)
//!        q/k/v = h·W{q,k,v} + b            （不分组，4 头 × 128）
//!        x = x + LinearOut(softmax(qkᵀ/√128)·v) + FSMN(v)
//!        x = x + fc2(relu(fc1(LayerNorm(x))))
//!        第 50 层、第 70 层末尾各接一个 LayerNorm
//!   → 512 维
//! ```
//!
//! 与官方实现对齐的要点：
//!
//! - **注意力是整段双向的**（不像 Qwen3-ASR 那样按 104 token 分窗），所以没有掩码；
//!   单条语音不进 padding 帧，掩码恒为全 1，这里直接省掉。
//! - **FSMN 跑在 v 上、加在注意力输出之后**：`att_outs + fsmn_memory`，其中
//!   `fsmn_memory = depthwise_conv(v) + v`。卷积核 11、左右各补 5 帧。
//! - 第 0 层的输入维度是 560，与 hidden 512 不等，因此**跳连在注意力和 FFN 两处
//!   都不加**（`residual = None`）。
//! - 位置编码用 FunASR 的 `SinusoidalPositionEncoder`：`sin` 在前半、`cos` 在后半，
//!   时间尺度 `ln(10000)/(d/2-1)`，并且**位置从 1 开始**。
//! - 50 层"转写层"之后接 20 层"时间戳预测层"，接在投影层之前——官方实现把编码器
//!   输出（含 tp 层）直接喂给 adaptor，所以这 20 层不能跳过。
//!
//! 投影层（`model.multi_modal_projector`）：
//! `linear_1: 512→2048` → relu → `linear_2: 2048→1024` → 2 层普通 transformer
//! （1024 维、8 头、FFN 只有 256、**没有 FSMN**）→ 1024 维，直接当 embedding 用。

use candle_core::{Device, Module, Tensor};
use candle_nn::{
    conv1d_no_bias, layer_norm, linear, ops::softmax_last_dim, Conv1d, Conv1dConfig, LayerNorm,
    Linear, VarBuilder,
};

use super::config::{AdaptorConfig, AudioEncoderConfig};
use crate::InferError;

/// 权重前缀：编码器、投影层（与 transformers 的 `fun_asr_nano` 布局一致）。
const AUDIO_TOWER: &str = "model.audio_tower";
const PROJECTOR: &str = "model.multi_modal_projector";

/// 一层 SAN-M（转写层、时间戳层、adaptor 层共用）。
struct SanmLayer {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    o_proj: Linear,
    /// `None` = adaptor 层（无 FSMN）。
    fsmn: Option<Conv1d>,
    fc1: Linear,
    fc2: Linear,
    input_ln: LayerNorm,
    post_ln: LayerNorm,
    final_ln: Option<LayerNorm>,
    heads: usize,
    head_dim: usize,
    hidden: usize,
    /// 输入维度：第 0 层是 560，其余等于 `hidden`。
    in_dim: usize,
    /// FSMN 左右补零宽度（`(kernel-1)/2`）。
    fsmn_pad: usize,
}

/// 一层的形状参数（转写层、时间戳层、adaptor 层三者共用一套实现）。
#[derive(Clone, Copy)]
struct LayerShape {
    in_dim: usize,
    hidden: usize,
    heads: usize,
    ffn: usize,
}

impl SanmLayer {
    fn load(
        vb: &VarBuilder,
        shape: LayerShape,
        eps: f64,
        fsmn_kernel: Option<usize>,
        add_norm: bool,
    ) -> Result<Self, InferError> {
        let LayerShape {
            in_dim,
            hidden,
            heads,
            ffn,
        } = shape;
        if !hidden.is_multiple_of(heads) {
            return Err(InferError::Config(format!(
                "hidden {hidden} 不能被头数 {heads} 整除"
            )));
        }
        let attn_dim = hidden;
        let vb_attn = vb.pp("self_attn");
        let fsmn = match fsmn_kernel {
            None => None,
            Some(k) => {
                if k.is_multiple_of(2) {
                    return Err(InferError::Config(format!("FSMN 卷积核 {k} 必须是奇数")));
                }
                // 深度卷积：groups = hidden，权重形状 [hidden, 1, k]
                let cfg = Conv1dConfig {
                    groups: hidden,
                    ..Default::default()
                };
                Some(conv1d_no_bias(
                    hidden,
                    hidden,
                    k,
                    cfg,
                    vb_attn.pp("fsmn.conv"),
                )?)
            }
        };
        Ok(Self {
            q_proj: linear(in_dim, attn_dim, vb_attn.pp("q_proj"))?,
            k_proj: linear(in_dim, attn_dim, vb_attn.pp("k_proj"))?,
            v_proj: linear(in_dim, attn_dim, vb_attn.pp("v_proj"))?,
            o_proj: linear(attn_dim, hidden, vb_attn.pp("o_proj"))?,
            fsmn,
            fc1: linear(hidden, ffn, vb.pp("mlp.fc1"))?,
            fc2: linear(ffn, hidden, vb.pp("mlp.fc2"))?,
            input_ln: layer_norm(in_dim, eps, vb.pp("input_layernorm"))?,
            post_ln: layer_norm(hidden, eps, vb.pp("post_attention_layernorm"))?,
            final_ln: if add_norm {
                Some(layer_norm(hidden, eps, vb.pp("final_layernorm"))?)
            } else {
                None
            },
            heads,
            head_dim: hidden / heads,
            hidden,
            in_dim,
            fsmn_pad: fsmn_kernel.map(|k| (k - 1) / 2).unwrap_or(0),
        })
    }

    /// `x`: `[T, in_dim]` → `[T, hidden]`。
    fn forward(&self, x: &Tensor) -> Result<Tensor, InferError> {
        let t = x.dims()[0];
        // 第 0 层 in_dim(560) ≠ hidden(512)，两处跳连都不加。
        let residual = if self.in_dim == self.hidden {
            Some(x.clone())
        } else {
            None
        };

        let h = self.input_ln.forward(x)?;
        let q = self.q_proj.forward(&h)?;
        let k = self.k_proj.forward(&h)?;
        // FSMN 用的是**没按头切开**的 v。
        let v = self.v_proj.forward(&h)?;

        let q = q
            .reshape((t, self.heads, self.head_dim))?
            .transpose(0, 1)?
            .contiguous()?;
        let k = k
            .reshape((t, self.heads, self.head_dim))?
            .transpose(0, 1)?
            .contiguous()?;
        let vh = v
            .reshape((t, self.heads, self.head_dim))?
            .transpose(0, 1)?
            .contiguous()?;

        let scale = 1.0 / (self.head_dim as f64).sqrt();
        let scores = (q.matmul(&k.transpose(1, 2)?.contiguous()?)? * scale)?;
        let probs = softmax_last_dim(&scores)?;
        let attn = probs
            .matmul(&vh)?
            .transpose(0, 1)?
            .contiguous()?
            .reshape((t, self.hidden))?;
        let mut out = self.o_proj.forward(&attn)?;

        if let Some(conv) = &self.fsmn {
            out = (&out + &self.fsmn_forward(conv, &v)?)?;
        }

        let x = match residual {
            Some(r) => (&r + &out)?,
            None => out,
        };

        let h = self.post_ln.forward(&x)?;
        let h = self.fc2.forward(&self.fc1.forward(&h)?.relu()?)?;
        let x = (&x + &h)?;
        match &self.final_ln {
            Some(ln) => Ok(ln.forward(&x)?),
            None => Ok(x),
        }
    }

    /// `depthwise_conv(pad(v)) + v`，`[T, hidden]`。
    fn fsmn_forward(&self, conv: &Conv1d, v: &Tensor) -> Result<Tensor, InferError> {
        let pad = self.fsmn_pad;
        let x = v.transpose(0, 1)?.contiguous()?; // [C, T]
        let x = if pad > 0 {
            x.pad_with_zeros(1, pad, pad)?
        } else {
            x
        };
        let x = conv.forward(&x.unsqueeze(0)?)?; // [1, C, T]
        let x = x.squeeze(0)?.transpose(0, 1)?.contiguous()?; // [T, C]
        Ok((&x + v)?)
    }
}

/// 音频编码器 + 投影层。
pub struct Encoder {
    layers: Vec<SanmLayer>,
    projector_1: Linear,
    projector_2: Linear,
    adaptor: Vec<SanmLayer>,
    /// `[max_pos, hidden]`，对应位置 1..=max_pos。
    pe: Tensor,
    scaling: f64,
    in_dim: usize,
    out_dim: usize,
}

impl Encoder {
    pub fn load(
        vb: &VarBuilder,
        audio: &AudioEncoderConfig,
        adaptor: &AdaptorConfig,
        dev: &Device,
    ) -> Result<Self, InferError> {
        let in_dim = audio.input_size();
        let vb_audio = vb.pp(AUDIO_TOWER);

        let transcribe_layers = audio
            .num_hidden_layers
            .checked_sub(audio.num_timestamp_prediction_layers)
            .ok_or_else(|| {
                InferError::Config(format!(
                    "层数 {} 小于时间戳层数 {}",
                    audio.num_hidden_layers, audio.num_timestamp_prediction_layers
                ))
            })?;
        if transcribe_layers == 0 {
            return Err(InferError::Config("转写层数为 0".into()));
        }
        // 只有最后一个转写层和最后一个时间戳层带 LayerNorm。
        let norm_at = [transcribe_layers - 1, audio.num_hidden_layers - 1];

        let mut layers = Vec::with_capacity(audio.num_hidden_layers);
        for i in 0..audio.num_hidden_layers {
            layers.push(SanmLayer::load(
                &vb_audio.pp(format!("layers.{i}")),
                LayerShape {
                    in_dim: if i == 0 { in_dim } else { audio.hidden_size },
                    hidden: audio.hidden_size,
                    heads: audio.attention_heads,
                    ffn: audio.intermediate_size,
                },
                audio.layer_norm_eps,
                Some(audio.fsmn_kernel_size),
                norm_at.contains(&i),
            )?);
        }

        let vb_proj = vb.pp(PROJECTOR);
        let projector_1 = linear(
            audio.hidden_size,
            adaptor.projector_hidden_size,
            vb_proj.pp("linear_1"),
        )?;
        let projector_2 = linear(
            adaptor.projector_hidden_size,
            adaptor.hidden_size,
            vb_proj.pp("linear_2"),
        )?;

        let mut adaptor_layers = Vec::with_capacity(adaptor.num_hidden_layers);
        for i in 0..adaptor.num_hidden_layers {
            adaptor_layers.push(SanmLayer::load(
                &vb_proj.pp(format!("layers.{i}")),
                LayerShape {
                    in_dim: adaptor.hidden_size,
                    hidden: adaptor.hidden_size,
                    heads: adaptor.attention_heads,
                    ffn: adaptor.intermediate_size,
                },
                adaptor.layer_norm_eps,
                None,
                false,
            )?);
        }

        // 位置编码的长度是 hidden 的倍数，但**宽度是 input_size**（560）——第一层之前
        // 特征还没投影到 512 维。官方 `FunAsrNanoPositionEmbedding(max_pos, input_size)`。
        let pe = sinusoidal_positions(1, audio.max_position_embeddings, in_dim, dev)?;

        Ok(Self {
            layers,
            projector_1,
            projector_2,
            adaptor: adaptor_layers,
            pe,
            scaling: (audio.hidden_size as f64).sqrt(),
            in_dim,
            out_dim: adaptor.hidden_size,
        })
    }

    /// 音频特征维度（LFR 之后，560）。
    pub fn in_dim(&self) -> usize {
        self.in_dim
    }

    /// 输出维度（文本模型 hidden，1024）。
    pub fn out_dim(&self) -> usize {
        self.out_dim
    }

    /// 编码器接受的最大帧数（受位置编码表长度限制）。
    pub fn max_frames(&self) -> usize {
        self.pe.dims()[0]
    }

    /// `[T, 560]` → `[T, 1024]`（音频 embedding，可直接拼进文本 embedding 序列）。
    pub fn forward(&self, feats: &Tensor) -> Result<Tensor, InferError> {
        let (t, d) = feats.dims2()?;
        if d != self.in_dim {
            return Err(InferError::Config(format!(
                "音频特征维度 {d} 与模型期望的 {} 不一致",
                self.in_dim
            )));
        }
        if t == 0 {
            return Err(InferError::EmptyAudio);
        }
        if t > self.max_frames() {
            return Err(InferError::AudioTooLong {
                secs: t as f32 * 0.06,
                max: self.max_frames() as f32 * 0.06,
            });
        }

        let pe = self.pe.narrow(0, 0, t)?.to_dtype(feats.dtype())?;
        let mut x = (feats * self.scaling)?.broadcast_add(&pe)?;
        for layer in &self.layers {
            x = layer.forward(&x)?;
        }

        let x = self.projector_1.forward(&x)?.relu()?;
        let mut x = self.projector_2.forward(&x)?;
        for layer in &self.adaptor {
            x = layer.forward(&x)?;
        }
        Ok(x)
    }
}

/// FunASR `SinusoidalPositionEncoder`：`sin` 在前半、`cos` 在后半，不交织。
///
/// 时间尺度是 `ln(10000)/(d/2-1)`（比标准 transformer 少一格），位置从 `start` 起。
fn sinusoidal_positions(
    start: usize,
    n_pos: usize,
    d_model: usize,
    dev: &Device,
) -> Result<Tensor, InferError> {
    let half = d_model / 2;
    if half < 2 {
        return Err(InferError::Config("d_model 太小".into()));
    }
    let log_timescale = 10_000f64.ln() / (half - 1) as f64;
    let inv: Vec<f32> = (0..half)
        .map(|d| (-(d as f64) * log_timescale).exp() as f32)
        .collect();

    let mut data = vec![0f32; n_pos * d_model];
    for p in 0..n_pos {
        let pos = (start + p) as f32;
        for d in 0..half {
            let angle = pos * inv[d];
            data[p * d_model + d] = angle.sin();
            data[p * d_model + half + d] = angle.cos();
        }
    }
    Ok(Tensor::from_vec(data, (n_pos, d_model), dev)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::DType;

    fn zeros() -> VarBuilder<'static> {
        VarBuilder::zeros(DType::F32, &Device::Cpu)
    }

    fn layer(in_dim: usize, hidden: usize, fsmn: bool) -> SanmLayer {
        SanmLayer::load(
            &zeros(),
            LayerShape {
                in_dim,
                hidden,
                heads: 4,
                ffn: 64,
            },
            1e-5,
            fsmn.then_some(11),
            false,
        )
        .unwrap()
    }

    #[test]
    fn zero_weights_leave_square_layer_unchanged() {
        // 权重全零时注意力与 FFN 都输出 0，跳连让整层变成恒等映射。
        let l = layer(32, 32, true);
        let x = Tensor::from_vec(
            (0..32 * 5).map(|i| i as f32 * 0.01).collect::<Vec<_>>(),
            (5, 32),
            &Device::Cpu,
        )
        .unwrap();
        let y = l.forward(&x).unwrap();
        assert_eq!(y.dims(), &[5, 32]);
        let (a, b) = (x.flatten_all().unwrap(), y.flatten_all().unwrap());
        let (a, b) = (a.to_vec1::<f32>().unwrap(), b.to_vec1::<f32>().unwrap());
        for (u, v) in a.iter().zip(b.iter()) {
            assert!((u - v).abs() < 1e-6, "{u} != {v}");
        }
    }

    #[test]
    fn first_layer_drops_residual_when_dims_differ() {
        // 560 → 512 的第一层没有跳连：零权重时输出全 0，但形状是 hidden。
        let l = layer(560, 32, true);
        let x = Tensor::ones((7, 560), DType::F32, &Device::Cpu).unwrap();
        let y = l.forward(&x).unwrap();
        assert_eq!(y.dims(), &[7, 32]);
        assert!(y
            .flatten_all()
            .unwrap()
            .to_vec1::<f32>()
            .unwrap()
            .iter()
            .all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn adaptor_layer_has_no_fsmn_and_keeps_shape() {
        let l = layer(32, 32, false);
        assert!(l.fsmn.is_none());
        let x = Tensor::ones((3, 32), DType::F32, &Device::Cpu).unwrap();
        assert_eq!(l.forward(&x).unwrap().dims(), &[3, 32]);
    }

    #[test]
    fn positional_encoding_starts_at_one() {
        // 位置 1 的第 0 维是 sin(inv[0])，不是 0（位置从 1 起）。
        let pe = sinusoidal_positions(1, 4, 8, &Device::Cpu).unwrap();
        let v = pe.to_vec2::<f32>().unwrap();
        assert_eq!(v.len(), 4);
        assert!(v[0][0].abs() > 1e-3);
        // 余弦在后半，位置 1、频率 1 时 cos(1) ≈ 0.5403
        assert!((v[0][4] - 1f32.cos()).abs() < 1e-5);
        // 前半是 sin(1) ≈ 0.8415
        assert!((v[0][0] - 1f32.sin()).abs() < 1e-5);
        // 位置递增 → sin 递增（小角度区间）
        assert!(v[1][0] > v[0][0]);
    }

    #[test]
    fn rejects_dimension_mismatch_and_oversize() {
        let audio = AudioEncoderConfig {
            num_mel_bins: 80,
            num_stacked_frames: 7,
            hidden_size: 16,
            attention_heads: 4,
            intermediate_size: 32,
            num_hidden_layers: 2,
            num_timestamp_prediction_layers: 1,
            layer_norm_eps: 1e-5,
            fsmn_kernel_size: 11,
            max_position_embeddings: 8,
        };
        let adaptor = AdaptorConfig {
            hidden_size: 24,
            attention_heads: 4,
            intermediate_size: 6,
            num_hidden_layers: 1,
            layer_norm_eps: 1e-5,
            projector_hidden_size: 32,
        };
        let enc = Encoder::load(&zeros(), &audio, &adaptor, &Device::Cpu).unwrap();
        assert_eq!(enc.in_dim(), 560);
        assert_eq!(enc.out_dim(), 24);

        let bad = Tensor::zeros((2, 123), DType::F32, &Device::Cpu).unwrap();
        assert!(matches!(enc.forward(&bad), Err(InferError::Config(_))));
        let long = Tensor::zeros((9, 560), DType::F32, &Device::Cpu).unwrap();
        assert!(matches!(
            enc.forward(&long),
            Err(InferError::AudioTooLong { .. })
        ));
        let ok = Tensor::zeros((4, 560), DType::F32, &Device::Cpu).unwrap();
        assert_eq!(enc.forward(&ok).unwrap().dims(), &[4, 24]);
    }
}
