//! Whisper 音频编码器：conv stem + 正弦位置表 + N 层 Pre-LN transformer。
//!
//! ```text
//! log-mel [mel, 3000]
//!   → conv1(k=3, s=1, p=1) + GELU      [d, 3000]
//!   → conv2(k=3, s=2, p=1) + GELU      [d, 1500]   （每 20 ms 一个 token）
//!   → + embed_positions（正弦表，权重里直接存着）
//!   → N 层：LN → 自注意力（无掩码）→ 残差；LN → fc1 → GELU → fc2 → 残差
//!   → layer_norm
//! ```
//!
//! 与 Qwen3-ASR 音频塔的差别（别把两者的位置编码习惯搞混）：
//!
//! - 这里是**整段双向注意力**，1500 个 token 互相可见；Qwen3 是按 8 秒窗口局部注意。
//! - 位置是**跨整段递增**的正弦表（1500 格，20 ms 一格）；Qwen3 每个 1 秒 chunk
//!   复用同一张 13 行表。
//! - 卷积是**整段一次做**（stride 2 一次到底），Qwen3 是按 100 帧 chunk 分别做。
//!
//! 权重前缀是 transformers 的 `model.encoder.*`（`conv1/conv2/embed_positions/
//! layers.{i}/layer_norm`）。位置表虽然是正弦函数生成的，但 HF 权重里存成了
//! 张量（1500×d），这里直接读权重，不在 Rust 里重算——重算需要和 HF 的
//! `sinusoids` 公式逐位对齐（`max_period=10000`、`sin/cos` 交替、`exp` 的写法），
//! 读表能省掉一整类「看起来对但差 1e-3」的问题。

use candle_core::{Module, Tensor};
use candle_nn::{
    conv1d, layer_norm, ops::softmax_last_dim, Conv1d, Conv1dConfig, LayerNorm, Linear, VarBuilder,
};

use super::config::{Activation, WhisperConfig};
use crate::InferError;

/// 编码器层堆叠。
pub struct Encoder {
    conv1: Conv1d,
    conv2: Conv1d,
    pos: Tensor,
    layers: Vec<EncLayer>,
    norm: LayerNorm,
    cfg: WhisperConfig,
}

struct EncLayer {
    attn: EncAttn,
    ln1: LayerNorm,
    ln2: LayerNorm,
    fc1: Linear,
    fc2: Linear,
}

struct EncAttn {
    q: Linear,
    k: Linear,
    v: Linear,
    out: Linear,
    heads: usize,
    head_dim: usize,
}

impl Encoder {
    pub fn load(vb: &VarBuilder, cfg: &WhisperConfig) -> Result<Self, InferError> {
        let vb = vb.pp("model.encoder");
        let d = cfg.d_model;

        let conv_cfg = Conv1dConfig {
            padding: 1,
            stride: 1,
            ..Default::default()
        };
        let conv1 = conv1d(cfg.num_mel_bins, d, 3, conv_cfg, vb.pp("conv1"))?;
        let conv_cfg = Conv1dConfig {
            padding: 1,
            stride: 2,
            ..Default::default()
        };
        let conv2 = conv1d(d, d, 3, conv_cfg, vb.pp("conv2"))?;

        let pos = vb.get((cfg.max_source_positions, d), "embed_positions.weight")?;

        let mut layers = Vec::with_capacity(cfg.encoder_layers);
        for i in 0..cfg.encoder_layers {
            layers.push(EncLayer::new(cfg, vb.pp("layers").pp(i))?);
        }
        let norm = layer_norm(d, 1e-5, vb.pp("layer_norm"))?;

        Ok(Self {
            conv1,
            conv2,
            pos,
            layers,
            norm,
            cfg: cfg.clone(),
        })
    }

    pub fn config(&self) -> &WhisperConfig {
        &self.cfg
    }

    /// `mel`：`[num_mel_bins, 3000]` → `[1, 1500, d_model]`。
    pub fn forward(&self, mel: &Tensor) -> Result<Tensor, InferError> {
        let (bins, frames) = mel.dims2().map_err(|_| {
            InferError::Config(format!(
                "编码器输入应为 [num_mel_bins, 3000]，实际 {:?}",
                mel.dims()
            ))
        })?;
        if bins != self.cfg.num_mel_bins || frames != super::frontend::WINDOW_FRAMES {
            return Err(InferError::Config(format!(
                "编码器输入应为 [{}, {}]，实际 [{bins}, {frames}]",
                self.cfg.num_mel_bins,
                super::frontend::WINDOW_FRAMES
            )));
        }

        let x = mel.unsqueeze(0)?;
        let x = apply_activation(&self.conv1.forward(&x)?, self.cfg.activation)?;
        let x = apply_activation(&self.conv2.forward(&x)?, self.cfg.activation)?;
        // [1, d, 1500] → [1, 1500, d]
        let x = x.transpose(1, 2)?.contiguous()?;

        let (_, positions, _) = x.dims3()?;
        if positions > self.cfg.max_source_positions {
            return Err(InferError::Config(format!(
                "卷积输出 {positions} 个位置，超出位置表 {}",
                self.cfg.max_source_positions
            )));
        }
        let pos = self.pos.narrow(0, 0, positions)?;
        let mut h = x.broadcast_add(&pos)?;

        for layer in &self.layers {
            h = layer.forward(&h, self.cfg.activation)?;
        }
        Ok(self.norm.forward(&h)?)
    }
}

impl EncLayer {
    fn new(cfg: &WhisperConfig, vb: VarBuilder) -> Result<Self, InferError> {
        let d = cfg.d_model;
        Ok(Self {
            attn: EncAttn::new(d, cfg.encoder_heads, vb.pp("self_attn"))?,
            ln1: layer_norm(d, 1e-5, vb.pp("self_attn_layer_norm"))?,
            ln2: layer_norm(d, 1e-5, vb.pp("final_layer_norm"))?,
            fc1: candle_nn::linear(d, cfg.encoder_ffn_dim, vb.pp("fc1"))?,
            fc2: candle_nn::linear(cfg.encoder_ffn_dim, d, vb.pp("fc2"))?,
        })
    }

    fn forward(&self, x: &Tensor, act: Activation) -> Result<Tensor, InferError> {
        // Pre-LN：LN → 子层 → 加回**子层之前**的输入。
        let h = self.attn.forward(&self.ln1.forward(x)?)?;
        let x = (x + h)?;
        let h = self.fc2.forward(&apply_activation(
            &self.fc1.forward(&self.ln2.forward(&x)?)?,
            act,
        )?)?;
        Ok((&x + h)?)
    }
}

impl EncAttn {
    fn new(d_model: usize, heads: usize, vb: VarBuilder) -> Result<Self, InferError> {
        Ok(Self {
            q: candle_nn::linear(d_model, d_model, vb.pp("q_proj"))?,
            // key 投影**没有 bias**：这是 OpenAI 原版 `MultiHeadAttention` 的约定
            // （`self.key = nn.Linear(..., bias=False)`），query/value 有。
            k: candle_nn::linear_no_bias(d_model, d_model, vb.pp("k_proj"))?,
            v: candle_nn::linear(d_model, d_model, vb.pp("v_proj"))?,
            out: candle_nn::linear(d_model, d_model, vb.pp("out_proj"))?,
            heads,
            head_dim: d_model / heads,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor, InferError> {
        let (b, l, _) = x.dims3()?;
        let h = self.heads;
        let d = self.head_dim;

        let q = self
            .q
            .forward(x)?
            .reshape((b, l, h, d))?
            .transpose(1, 2)?
            .contiguous()?;
        let k = self
            .k
            .forward(x)?
            .reshape((b, l, h, d))?
            .transpose(1, 2)?
            .contiguous()?;
        let v = self
            .v
            .forward(x)?
            .reshape((b, l, h, d))?
            .transpose(1, 2)?
            .contiguous()?;

        // Whisper 官方把 scaling 乘在 q 上；等价于乘在 logits 上。
        let scale = 1.0 / (d as f64).sqrt();
        let q = (q * scale)?;
        let scores = q.matmul(&k.transpose(2, 3)?.contiguous()?)?;
        let probs = softmax_last_dim(&scores)?;
        let out = probs
            .matmul(&v)?
            .transpose(1, 2)?
            .contiguous()?
            .reshape((b, l, h * d))?;
        Ok(self.out.forward(&out)?)
    }
}

/// 前馈激活。whisper 是精确 erf 版 gelu（`Tensor::gelu` 是 tanh 近似，别用错）。
pub(crate) fn apply_activation(x: &Tensor, act: Activation) -> Result<Tensor, InferError> {
    Ok(match act {
        Activation::Gelu => x.gelu_erf()?,
        Activation::GeluTanh => x.gelu()?,
        Activation::Silu => x.silu()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::whisper::config::WhisperConfig;
    use candle_core::{DType, Device};
    use serde_json::json;

    fn tiny_cfg() -> WhisperConfig {
        WhisperConfig::from_json(&json!({
            "model_type": "whisper", "d_model": 16,
            "encoder_layers": 1, "decoder_layers": 1,
            "encoder_attention_heads": 2, "decoder_attention_heads": 2,
            "encoder_ffn_dim": 32, "decoder_ffn_dim": 32, "vocab_size": 64,
            "num_mel_bins": 80, "max_source_positions": 1500,
            "max_target_positions": 16, "activation_function": "gelu",
            "decoder_start_token_id": 1, "eos_token_id": 2, "pad_token_id": 2
        }))
        .unwrap()
    }

    /// 全零权重也能验证形状链：卷积下采样、位置表广播、层输出维度。
    #[test]
    fn forward_shapes_match_whisper() {
        let dev = Device::Cpu;
        let cfg = tiny_cfg();
        let vb = VarBuilder::zeros(DType::F32, &dev);
        let enc = Encoder::load(&vb, &cfg).unwrap();
        let mel = Tensor::zeros((80, 3000), DType::F32, &dev).unwrap();
        let out = enc.forward(&mel).unwrap();
        assert_eq!(out.dims(), &[1, 1500, 16]);
    }

    #[test]
    fn rejects_wrong_mel_shape() {
        let dev = Device::Cpu;
        let vb = VarBuilder::zeros(DType::F32, &dev);
        let enc = Encoder::load(&vb, &tiny_cfg()).unwrap();
        let bad = Tensor::zeros((128, 3000), DType::F32, &dev).unwrap();
        assert!(enc.forward(&bad).is_err());
        let short = Tensor::zeros((80, 160), DType::F32, &dev).unwrap();
        assert!(enc.forward(&short).is_err());
    }
}
