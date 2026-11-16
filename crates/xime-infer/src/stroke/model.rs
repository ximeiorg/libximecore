//! StrokeTransformer —— 笔画轨迹分类模型（candle 实现）。
//!
//! 对应训练仓 `ochwpro/model.py`：3 层 Pre-Norm Transformer 编码器 + [CLS] 分类头，
//! 输入 `(B, T, 5)`，输出 `(B, 7356)` logits。推理时 dropout 为恒等，故不建模块。

use candle_core::{DType, Device, Result, Tensor};
use candle_nn::{self as nn, Module, VarBuilder};

/// 模型结构参数（默认值与 `model_meta.json` 一致）。
#[derive(Clone, Debug, PartialEq)]
pub struct StrokeConfig {
    pub num_classes: usize,
    pub d_model: usize,
    pub nhead: usize,
    pub num_layers: usize,
    pub dim_feedforward: usize,
    pub max_seq_len: usize,
}

impl Default for StrokeConfig {
    fn default() -> Self {
        Self {
            num_classes: 7356,
            d_model: 192,
            nhead: 4,
            num_layers: 3,
            dim_feedforward: 384,
            max_seq_len: 512,
        }
    }
}

/// 从 safetensors 字节装载模型（PyTorch 命名；fp16 权重会转成 f32 计算）。
pub fn load(weights: &[u8], config: &StrokeConfig, device: &Device) -> Result<StrokeTransformer> {
    // 训练仓发布的是 fp16 safetensors；VarBuilder 按目标 dtype 读取时会做转换。
    // CPU 上的 f16 算子支持不完整，统一用 f32 计算。
    let vb = VarBuilder::from_buffered_safetensors(weights.to_vec(), DType::F32, device)?;
    StrokeTransformer::load_with_vb(vb, config)
}

struct PositionalEncoding {
    pe: Tensor,
}

impl PositionalEncoding {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let n = x.dims()[1];
        x.broadcast_add(&self.pe.narrow(1, 0, n)?)
    }
}

struct SelfAttention {
    in_proj_weight: Tensor,
    in_proj_bias: Tensor,
    out_proj: nn::Linear,
    nhead: usize,
}

impl SelfAttention {
    fn load(vb: VarBuilder, config: &StrokeConfig) -> Result<Self> {
        Ok(Self {
            in_proj_weight: vb.get((3 * config.d_model, config.d_model), "in_proj_weight")?,
            in_proj_bias: vb.get(3 * config.d_model, "in_proj_bias")?,
            out_proj: nn::linear(config.d_model, config.d_model, vb.pp("out_proj"))?,
            nhead: config.nhead,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, t, d) = x.dims3()?;
        let head_dim = d / self.nhead;
        let scale = 1.0 / (head_dim as f64).sqrt();

        // 融合 QKV 投影：x @ W^T + b
        let qkv = x
            .reshape((b * t, d))?
            .matmul(&self.in_proj_weight.t()?)?
            .reshape((b, t, 3 * d))?
            .broadcast_add(&self.in_proj_bias)?;

        // Metal 的 matmul 不接受非连续输入；permute 后必须 contiguous。
        let q = qkv
            .narrow(2, 0, d)?
            .reshape((b, t, self.nhead, head_dim))?
            .permute((0, 2, 1, 3))?
            .contiguous()?;
        let k = qkv
            .narrow(2, d, d)?
            .reshape((b, t, self.nhead, head_dim))?
            .permute((0, 2, 1, 3))?
            .contiguous()?;
        let v = qkv
            .narrow(2, 2 * d, d)?
            .reshape((b, t, self.nhead, head_dim))?
            .permute((0, 2, 1, 3))?
            .contiguous()?;

        // 与 PyTorch 一致：先按 1/sqrt(head_dim) 缩放 q，再做 QK^T。
        let attn = q.affine(scale, 0.0)?.matmul(&k.t()?.contiguous()?)?;
        let attn = candle_nn::ops::softmax(&attn, 3)?;
        let h = attn.matmul(&v)?;

        let h = h.permute((0, 2, 1, 3))?.reshape((b, t, d))?.contiguous()?;
        self.out_proj.forward(&h)
    }
}

struct TransformerLayer {
    self_attn: SelfAttention,
    linear1: nn::Linear,
    linear2: nn::Linear,
    norm1: nn::LayerNorm,
    norm2: nn::LayerNorm,
}

impl TransformerLayer {
    fn load(vb: VarBuilder, config: &StrokeConfig) -> Result<Self> {
        Ok(Self {
            self_attn: SelfAttention::load(vb.pp("self_attn"), config)?,
            linear1: nn::linear(config.d_model, config.dim_feedforward, vb.pp("linear1"))?,
            linear2: nn::linear(config.dim_feedforward, config.d_model, vb.pp("linear2"))?,
            norm1: nn::layer_norm(config.d_model, 1e-5, vb.pp("norm1"))?,
            norm2: nn::layer_norm(config.d_model, 1e-5, vb.pp("norm2"))?,
        })
    }

    /// Pre-Norm：`x + attn(norm1(x))`，再 `x + ffn(norm2(x))`。
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        // 注意：残差加的是 **norm 之前** 的 x，不是 norm1 的输出。
        let normed = self.norm1.forward(x)?;
        let h = x.add(&self.self_attn.forward(&normed)?)?;

        let h2 = self.norm2.forward(&h)?;
        let h2 = self.linear1.forward(&h2)?;
        let h2 = h2.gelu_erf()?;
        let h2 = self.linear2.forward(&h2)?;
        h.add(&h2)
    }
}

/// 笔画分类模型。
pub struct StrokeTransformer {
    input_proj: nn::Linear,
    pos_encoder: PositionalEncoding,
    cls_token: Tensor,
    layers: Vec<TransformerLayer>,
    norm: nn::LayerNorm,
    classifier_1: nn::Linear,
    classifier_4: nn::Linear,
    config: StrokeConfig,
}

impl StrokeTransformer {
    /// 用给定 `VarBuilder` 构建（单测用 `VarBuilder::zeros`）。
    pub(crate) fn load_with_vb(vb: VarBuilder, config: &StrokeConfig) -> Result<Self> {
        let input_proj = nn::linear(5, config.d_model, vb.pp("input_proj"))?;
        let pos_encoder = PositionalEncoding {
            pe: vb.get(
                (1, config.max_seq_len + 1, config.d_model),
                "pos_encoder.pe",
            )?,
        };
        let cls_token = vb.get((1, 1, config.d_model), "cls_token")?;

        let mut layers = Vec::with_capacity(config.num_layers);
        for i in 0..config.num_layers {
            layers.push(TransformerLayer::load(
                vb.pp(format!("transformer.layers.{i}")),
                config,
            )?);
        }

        Ok(Self {
            input_proj,
            pos_encoder,
            cls_token,
            layers,
            norm: nn::layer_norm(config.d_model, 1e-5, vb.pp("norm"))?,
            classifier_1: nn::linear(config.d_model, config.d_model / 2, vb.pp("classifier.1"))?,
            classifier_4: nn::linear(
                config.d_model / 2,
                config.num_classes,
                vb.pp("classifier.4"),
            )?,
            config: config.clone(),
        })
    }

    /// 前向：`(B, T, 5)` → `(B, num_classes)` logits。
    pub fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let b = x.dims()[0];
        let d = self.config.d_model;

        let mut h = self.input_proj.forward(x)?;
        let cls = self.cls_token.broadcast_as((b, 1, d))?;
        h = Tensor::cat(&[&cls, &h], 1)?;
        h = self.pos_encoder.forward(&h)?;

        for layer in &self.layers {
            h = layer.forward(&h)?;
        }

        // [CLS] 位 → LayerNorm → 2 层 MLP 分类头
        let cls_out = h.narrow(1, 0, 1)?.squeeze(1)?;
        let h = self.norm.forward(&cls_out)?;
        let h = self.classifier_1.forward(&h)?;
        let h = h.gelu_erf()?;
        self.classifier_4.forward(&h)
    }

    pub fn num_classes(&self) -> usize {
        self.config.num_classes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_model(seq_len: usize) -> (StrokeTransformer, Device) {
        let dev = Device::Cpu;
        let vb = VarBuilder::zeros(DType::F32, &dev);
        let cfg = StrokeConfig::default();
        let model = StrokeTransformer::load_with_vb(vb, &cfg).unwrap();
        assert_eq!(model.num_classes(), 7356);
        assert_eq!(model.layers.len(), 3);
        let _ = seq_len;
        (model, dev)
    }

    #[test]
    fn default_config_matches_training_meta() {
        let c = StrokeConfig::default();
        assert_eq!(c.num_classes, 7356);
        assert_eq!(c.d_model, 192);
        assert_eq!(c.nhead, 4);
        assert_eq!(c.num_layers, 3);
        assert_eq!(c.dim_feedforward, 384);
        assert_eq!(c.max_seq_len, 512);
        assert_eq!(c.d_model % c.nhead, 0);
    }

    #[test]
    fn forward_shapes_for_various_seq_lens() {
        let (model, dev) = zero_model(0);
        for &t in &[1usize, 5, 50, 200] {
            let x = Tensor::zeros((1, t, 5), DType::F32, &dev).unwrap();
            let y = model.forward(&x).unwrap();
            assert_eq!(y.dims(), &[1, 7356], "seq_len={t}");
            assert!(y.to_vec2::<f32>().unwrap()[0].iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn batch_dim_is_supported() {
        let (model, dev) = zero_model(0);
        let x = Tensor::zeros((3, 12, 5), DType::F32, &dev).unwrap();
        assert_eq!(model.forward(&x).unwrap().dims(), &[3, 7356]);
    }

    #[test]
    fn softmax_sums_to_one() {
        let (model, dev) = zero_model(0);
        let x = Tensor::zeros((1, 20, 5), DType::F32, &dev).unwrap();
        let logits = model.forward(&x).unwrap();
        let probs = candle_nn::ops::softmax(&logits, 1).unwrap();
        let sum: f32 = probs.sum(1).unwrap().to_vec1::<f32>().unwrap()[0];
        assert!((sum - 1.0).abs() < 1e-4);
    }
}
