//! Whisper 文本解码器：学习式位置表 + 因果自注意力 + 交叉注意力 + KV cache。
//!
//! ```text
//! token ids → embed_tokens + embed_positions（448 行，学习出来的）
//!   → N 层：LN → 因果自注意力 → 残差
//!           LN → 交叉注意力（K/V 来自编码器输出）→ 残差
//!           LN → fc1 → GELU → fc2 → 残差
//!   → layer_norm → 与 embed_tokens **共享权重**的输出投影 → logits
//! ```
//!
//! 三个容易踩的点：
//!
//! 1. **输出投影没有独立权重**。transformers 里 `proj_out.weight` 和
//!    `embed_tokens.weight` 是同一张量（tied），safetensors 里只存了一份。
//!    这里复用 `Embedding` 的权重，缺了就是 66M 参数的随机初始化。
//! 2. **交叉注意力的 K/V 来自编码器输出**，与解码器隐藏状态无关；同一个 30 s
//!    窗口内它是常量，所以在 prefill 时算一次存起来，逐 token 解码时不再重算。
//!    官方实现每步都重算 1500 个位置的 K/V，那是纯浪费（4 层 turbo 也白算
//!    4×2×1500×d² 次乘加）。
//! 3. **解码器位置只有 448 格**，也是单次生成的硬上限（30 s 音频最多 448 个
//!    文本 token）。超过就不是「慢一点」而是越界。
//!
//! 自注意力的 KV cache 用 candle 的 `KvCache`（预分配 + `slice_set`），与
//! Qwen3-ASR 解码器同样的做法；`offset` 是 cache 里已有的 token 数。

use candle_core::{DType, Device, Module, Tensor};
use candle_nn::{
    embedding, kv_cache::KvCache, layer_norm, linear, linear_no_bias, ops::softmax_last_dim,
    Embedding, LayerNorm, Linear, VarBuilder,
};

use super::config::{Activation, WhisperConfig};
use super::encoder::apply_activation;
use crate::InferError;

pub struct Decoder {
    embed: Embedding,
    pos: Tensor,
    layers: Vec<DecLayer>,
    norm: LayerNorm,
    lm_head: Linear,
    cfg: WhisperConfig,
    activation: Activation,
}

struct DecLayer {
    self_attn: SelfAttn,
    cross_attn: CrossAttn,
    ln1: LayerNorm,
    ln2: LayerNorm,
    ln3: LayerNorm,
    fc1: Linear,
    fc2: Linear,
}

struct SelfAttn {
    q: Linear,
    k: Linear,
    v: Linear,
    out: Linear,
    cache: KvCache,
    heads: usize,
    head_dim: usize,
}

struct CrossAttn {
    q: Linear,
    k: Linear,
    v: Linear,
    out: Linear,
    heads: usize,
    head_dim: usize,
    /// prefill 时算好的 `([1,h,1500,d], [1,h,1500,d])`。
    kv: Option<(Tensor, Tensor)>,
}

impl Decoder {
    pub fn load(vb: &VarBuilder, cfg: &WhisperConfig) -> Result<Self, InferError> {
        let vb = vb.pp("model.decoder");
        let d = cfg.d_model;

        let embed = embedding(cfg.vocab_size, d, vb.pp("embed_tokens"))?;
        let pos = vb.get((cfg.max_target_positions, d), "embed_positions.weight")?;

        let mut layers = Vec::with_capacity(cfg.decoder_layers);
        for i in 0..cfg.decoder_layers {
            layers.push(DecLayer::new(cfg, vb.pp("layers").pp(i))?);
        }
        let norm = layer_norm(d, 1e-5, vb.pp("layer_norm"))?;

        // 绑定权重：HF 的 `proj_out` 与 `embed_tokens` 是同一张张量，safetensors
        // 里没有独立的 `proj_out.weight`。
        let lm_head = Linear::new(embed.embeddings().clone(), None);

        Ok(Self {
            embed,
            pos,
            layers,
            norm,
            lm_head,
            cfg: cfg.clone(),
            activation: cfg.activation,
        })
    }

    pub fn config(&self) -> &WhisperConfig {
        &self.cfg
    }

    /// `[1, seq]` → `[1, seq, d]`（含位置加法）。
    fn embed_ids(&self, ids: &Tensor, offset: usize) -> Result<Tensor, InferError> {
        let (_, seq) = ids.dims2()?;
        let mut h = self.embed.forward(ids)?;
        if self.cfg.scale_embedding {
            let scale = (self.cfg.d_model as f64).sqrt();
            h = (h * scale)?;
        }
        let pos = self.pos.narrow(0, offset, seq)?;
        Ok(h.broadcast_add(&pos)?)
    }

    /// 预填整段 prompt：跑完自注意力（因果掩码）+ 交叉注意力，返回最后一格 logits
    /// `[1, vocab]`。
    ///
    /// 同时把编码器输出的 K/V 投影算好缓存下来，供后续 [`Self::step`] 复用。
    pub fn prefill(&mut self, enc: &Tensor, prompt: &[u32]) -> Result<Tensor, InferError> {
        if prompt.is_empty() {
            return Err(InferError::Config("prompt 为空".into()));
        }
        if prompt.len() + 1 > self.cfg.max_target_positions {
            return Err(InferError::Config(format!(
                "prompt {} 个 token 已用满解码器位置表 {}",
                prompt.len(),
                self.cfg.max_target_positions
            )));
        }
        let (b, n_enc, _) = enc.dims3().map_err(|_| {
            InferError::Config(format!(
                "编码器输出应为 [1, 1500, d_model]，实际 {:?}",
                enc.dims()
            ))
        })?;
        if b != 1 {
            return Err(InferError::Config("batch 必须为 1".into()));
        }
        // 编码器输出直接当 K/V 的来源，长度就是位置表长度，不必额外校验。

        self.reset();
        for layer in &mut self.layers {
            layer.cross_attn.prepare(enc, n_enc)?;
        }

        let dev = self.embed.embeddings().device();
        let ids = Tensor::from_vec(prompt.to_vec(), (1, prompt.len()), dev)?;
        let mut h = self.embed_ids(&ids, 0)?;

        let mask = if prompt.len() > 1 {
            Some(causal_mask(prompt.len(), 0, dev, h.dtype())?)
        } else {
            None
        };
        for layer in &mut self.layers {
            h = layer.forward(&h, mask.as_ref(), self.activation)?;
        }
        self.project_last(&h, prompt.len())
    }

    /// 接着已预填的 cache 继续喂一段 token（位置从 cache 末尾算起），返回最后一格
    /// logits `[1, vocab]`。
    ///
    /// 语种检测的副产品：判定语种时已经用 `[<|startoftranscript|>]` 预填过一次，
    /// 交叉注意力的 K/V 与 sot 的自注意力 cache 都在，正式 prompt 只剩语种/task/
    /// `<|notimestamps|>` 三个 token。重跑一遍 `prefill` 会把 1500 个位置的
    /// K/V 投影再算一次（turbo 上约 20 GFLOP、CPU 上 2 s），这里直接续上。
    pub fn continue_prefill(&mut self, rest: &[u32]) -> Result<Tensor, InferError> {
        if rest.is_empty() {
            return Err(InferError::Config("续写 token 为空".into()));
        }
        let offset = self.cache_len();
        if offset == 0
            || self
                .layers
                .first()
                .is_none_or(|l| l.cross_attn.kv.is_none())
        {
            return Err(InferError::Config(
                "continue_prefill 之前必须先 prefill".into(),
            ));
        }
        let total = offset + rest.len();
        if total + 1 > self.cfg.max_target_positions {
            return Err(InferError::Config(format!(
                "prompt 总长 {total} 已用满解码器位置表 {}",
                self.cfg.max_target_positions
            )));
        }

        let dev = self.embed.embeddings().device();
        let ids = Tensor::from_vec(rest.to_vec(), (1, rest.len()), dev)?;
        let mut h = self.embed_ids(&ids, offset)?;
        let mask = causal_mask(rest.len(), offset, dev, h.dtype())?;
        for layer in &mut self.layers {
            h = layer.forward(&h, Some(&mask), self.activation)?;
        }
        self.project_last(&h, rest.len())
    }

    /// 自回归一步：喂一个 token（位置为 `offset`），返回 logits `[1, vocab]`。
    pub fn step(&mut self, token: u32, offset: usize) -> Result<Tensor, InferError> {
        if offset >= self.cfg.max_target_positions {
            return Err(InferError::Config(format!(
                "解码位置 {offset} 超出位置表 {}",
                self.cfg.max_target_positions
            )));
        }
        if self
            .layers
            .first()
            .is_none_or(|l| l.cross_attn.kv.is_none())
        {
            return Err(InferError::Config(
                "step 之前必须先调用 prefill（交叉注意力 K/V 未就绪）".into(),
            ));
        }
        let dev = self.embed.embeddings().device();
        let ids = Tensor::from_vec(vec![token], (1, 1), dev)?;
        let mut h = self.embed_ids(&ids, offset)?;
        for layer in &mut self.layers {
            h = layer.forward(&h, None, self.activation)?;
        }
        self.project_last(&h, 1)
    }

    fn project_last(&self, h: &Tensor, len: usize) -> Result<Tensor, InferError> {
        let last = self.norm.forward(h)?.narrow(1, len - 1, 1)?.squeeze(1)?;
        Ok(last.apply(&self.lm_head)?)
    }

    /// 清空自注意力 KV cache 与交叉注意力缓存。
    pub fn reset(&mut self) {
        for layer in &mut self.layers {
            layer.self_attn.cache.reset();
            layer.cross_attn.kv = None;
        }
    }

    /// 自注意力 cache 当前长度（调试/测试用）。
    pub fn cache_len(&self) -> usize {
        self.layers
            .first()
            .map(|l| l.self_attn.cache.current_seq_len())
            .unwrap_or(0)
    }
}

impl DecLayer {
    fn new(cfg: &WhisperConfig, vb: VarBuilder) -> Result<Self, InferError> {
        let d = cfg.d_model;
        Ok(Self {
            self_attn: SelfAttn::new(
                d,
                cfg.decoder_heads,
                cfg.max_target_positions,
                vb.pp("self_attn"),
            )?,
            cross_attn: CrossAttn::new(d, cfg.decoder_heads, vb.pp("encoder_attn"))?,
            ln1: layer_norm(d, 1e-5, vb.pp("self_attn_layer_norm"))?,
            ln2: layer_norm(d, 1e-5, vb.pp("encoder_attn_layer_norm"))?,
            ln3: layer_norm(d, 1e-5, vb.pp("final_layer_norm"))?,
            fc1: linear(d, cfg.decoder_ffn_dim, vb.pp("fc1"))?,
            fc2: linear(cfg.decoder_ffn_dim, d, vb.pp("fc2"))?,
        })
    }

    fn forward(
        &mut self,
        x: &Tensor,
        mask: Option<&Tensor>,
        act: Activation,
    ) -> Result<Tensor, InferError> {
        let h = self.self_attn.forward(&self.ln1.forward(x)?, mask)?;
        let x = (x + h)?;
        let h = self.cross_attn.forward(&self.ln2.forward(&x)?)?;
        let x = (&x + h)?;
        let h = self.fc2.forward(&apply_activation(
            &self.fc1.forward(&self.ln3.forward(&x)?)?,
            act,
        )?)?;
        Ok((&x + h)?)
    }
}

impl SelfAttn {
    fn new(
        d_model: usize,
        heads: usize,
        max_positions: usize,
        vb: VarBuilder,
    ) -> Result<Self, InferError> {
        Ok(Self {
            q: linear(d_model, d_model, vb.pp("q_proj"))?,
            // key 投影没有 bias（OpenAI 原版约定，与 q/v 不同）。
            k: linear_no_bias(d_model, d_model, vb.pp("k_proj"))?,
            v: linear(d_model, d_model, vb.pp("v_proj"))?,
            out: linear(d_model, d_model, vb.pp("out_proj"))?,
            // 上限由 `max_target_positions`（448）决定，预分配后每步只写自己那格。
            cache: KvCache::new(2, max_positions),
            heads,
            head_dim: d_model / heads,
        })
    }

    fn forward(&mut self, x: &Tensor, mask: Option<&Tensor>) -> Result<Tensor, InferError> {
        let (b, l, _) = x.dims3()?;
        let h = self.heads;
        let d = self.head_dim;
        let scale = 1.0 / (d as f64).sqrt();

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

        let (k, v) = self.cache.append(&k, &v)?;
        let q = (q * scale)?;
        let mut scores = q.matmul(&k.transpose(2, 3)?.contiguous()?)?;
        if let Some(m) = mask {
            scores = scores.broadcast_add(m)?;
        }
        let probs = softmax_last_dim(&scores)?;
        let out = probs
            .matmul(&v)?
            .transpose(1, 2)?
            .contiguous()?
            .reshape((b, l, h * d))?;
        Ok(self.out.forward(&out)?)
    }
}

impl CrossAttn {
    fn new(d_model: usize, heads: usize, vb: VarBuilder) -> Result<Self, InferError> {
        Ok(Self {
            q: linear(d_model, d_model, vb.pp("q_proj"))?,
            // 交叉注意力的 key 同样没有 bias。
            k: linear_no_bias(d_model, d_model, vb.pp("k_proj"))?,
            v: linear(d_model, d_model, vb.pp("v_proj"))?,
            out: linear(d_model, d_model, vb.pp("out_proj"))?,
            heads,
            head_dim: d_model / heads,
            kv: None,
        })
    }

    /// 把编码器输出投影成 K/V 并缓存（每个 prefill 一次）。
    fn prepare(&mut self, enc: &Tensor, n_enc: usize) -> Result<(), InferError> {
        let (b, _, _) = enc.dims3()?;
        let h = self.heads;
        let d = self.head_dim;
        let k = self
            .k
            .forward(enc)?
            .reshape((b, n_enc, h, d))?
            .transpose(1, 2)?
            .contiguous()?;
        let v = self
            .v
            .forward(enc)?
            .reshape((b, n_enc, h, d))?
            .transpose(1, 2)?
            .contiguous()?;
        self.kv = Some((k, v));
        Ok(())
    }

    fn forward(&mut self, x: &Tensor) -> Result<Tensor, InferError> {
        let (b, l, _) = x.dims3()?;
        let h = self.heads;
        let d = self.head_dim;
        let (k, v) = self
            .kv
            .as_ref()
            .ok_or_else(|| InferError::Config("交叉注意力 K/V 未初始化".into()))?;

        let q = self
            .q
            .forward(x)?
            .reshape((b, l, h, d))?
            .transpose(1, 2)?
            .contiguous()?;
        let scale = 1.0 / (d as f64).sqrt();
        let q = (q * scale)?;
        // 没有掩码：解码器每个位置都能看到整段音频。
        let scores = q.matmul(&k.transpose(2, 3)?.contiguous()?)?;
        let probs = softmax_last_dim(&scores)?;
        let out = probs
            .matmul(v)?
            .transpose(1, 2)?
            .contiguous()?
            .reshape((b, l, h * d))?;
        Ok(self.out.forward(&out)?)
    }
}

/// 因果掩码 `[1, 1, tgt, tgt+offset]`：`offset` 是 cache 里已有的 token 数，
/// 每个新位置能看到「全部旧 token + 自己及之前的新 token」。
fn causal_mask(
    tgt: usize,
    offset: usize,
    dev: &Device,
    dtype: DType,
) -> Result<Tensor, InferError> {
    let row = tgt + offset;
    let mut data = vec![0f32; tgt * row];
    for i in 0..tgt {
        for j in 0..row {
            if j > i + offset {
                data[i * row + j] = f32::NEG_INFINITY;
            }
        }
    }
    Ok(Tensor::from_vec(data, (1, 1, tgt, row), dev)?.to_dtype(dtype)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::whisper::config::WhisperConfig;
    use serde_json::json;

    fn cfg() -> WhisperConfig {
        WhisperConfig::from_json(&json!({
            "model_type": "whisper", "d_model": 16,
            "encoder_layers": 1, "decoder_layers": 2,
            "encoder_attention_heads": 2, "decoder_attention_heads": 2,
            "encoder_ffn_dim": 32, "decoder_ffn_dim": 32, "vocab_size": 64,
            "num_mel_bins": 80, "max_source_positions": 1500,
            "max_target_positions": 448, "activation_function": "gelu",
            "decoder_start_token_id": 1, "eos_token_id": 2, "pad_token_id": 2
        }))
        .unwrap()
    }

    #[test]
    fn prefill_and_step_shapes() {
        let dev = Device::Cpu;
        let vb = VarBuilder::zeros(DType::F32, &dev);
        let c = cfg();
        let mut dec = Decoder::load(&vb, &c).unwrap();
        let enc = Tensor::zeros((1, 1500, 16), DType::F32, &dev).unwrap();

        let logits = dec.prefill(&enc, &[1, 9, 10, 11]).unwrap();
        assert_eq!(logits.dims(), &[1, 64]);
        assert_eq!(dec.cache_len(), 4);

        for i in 0..3 {
            let logits = dec.step(7, 4 + i).unwrap();
            assert_eq!(logits.dims(), &[1, 64]);
        }
        assert_eq!(dec.cache_len(), 7);

        dec.reset();
        assert_eq!(dec.cache_len(), 0);
    }

    #[test]
    fn step_without_prefill_is_rejected() {
        let dev = Device::Cpu;
        let vb = VarBuilder::zeros(DType::F32, &dev);
        let mut dec = Decoder::load(&vb, &cfg()).unwrap();
        assert!(dec.step(1, 0).is_err());
    }

    #[test]
    fn prompt_longer_than_positions_is_rejected() {
        let dev = Device::Cpu;
        let vb = VarBuilder::zeros(DType::F32, &dev);
        let mut dec = Decoder::load(&vb, &cfg()).unwrap();
        let enc = Tensor::zeros((1, 1500, 16), DType::F32, &dev).unwrap();
        let prompt = vec![1u32; 448];
        assert!(dec.prefill(&enc, &prompt).is_err());
    }

    #[test]
    fn causal_mask_is_upper_triangular() {
        let m = causal_mask(3, 0, &Device::Cpu, DType::F32).unwrap();
        assert_eq!(m.dims(), &[1, 1, 3, 3]);
        let v = m.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert_eq!(v[0], 0.0);
        assert_eq!(v[1], f32::NEG_INFINITY);
        assert_eq!(v[4], 0.0);
        assert_eq!(v[5], f32::NEG_INFINITY);
        assert_eq!(v[8], 0.0);
    }

    #[test]
    fn causal_mask_with_offset_sees_prefix() {
        // 续写 2 个 token、cache 里已有 1 个：形状 [1,1,2,3]，旧 token 全可见。
        let m = causal_mask(2, 1, &Device::Cpu, DType::F32).unwrap();
        assert_eq!(m.dims(), &[1, 1, 2, 3]);
        let v = m.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert_eq!(&v[..3], &[0.0, 0.0, f32::NEG_INFINITY]); // 第一个新位置：sot + 自己
        assert_eq!(&v[3..], &[0.0, 0.0, 0.0]); // 第二个新位置能看到全部三个
    }

    #[test]
    fn continue_prefill_matches_full_prefill() {
        // 全零权重下两条路径的 logits 也必须一致：先 [sot] 再续 [lang, task, nots]
        // 与一次性 prefill 四个 token 等价。
        let dev = Device::Cpu;
        let vb = VarBuilder::zeros(DType::F32, &dev);
        let c = cfg();
        let enc = Tensor::zeros((1, 1500, 16), DType::F32, &dev).unwrap();
        let prompt = [1u32, 9, 10, 11];

        let mut a = Decoder::load(&vb, &c).unwrap();
        let full = a.prefill(&enc, &prompt).unwrap();

        let mut b = Decoder::load(&vb, &c).unwrap();
        let _ = b.prefill(&enc, &prompt[..1]).unwrap();
        let cont = b.continue_prefill(&prompt[1..]).unwrap();
        assert_eq!(cont.dims(), full.dims());
        assert_eq!(b.cache_len(), prompt.len());

        let fv = full.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        let cv = cont.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        for (x, y) in fv.iter().zip(cv.iter()) {
            assert!((x - y).abs() < 1e-6, "{x} vs {y}");
        }
    }
}
