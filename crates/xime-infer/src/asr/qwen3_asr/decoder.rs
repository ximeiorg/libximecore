//! Qwen3 文本解码器（GQA + RoPE + KV cache）。
//!
//! 自己实现而不用 `candle-transformers` 的 `qwen3`：ASR 需要在 prefill 时把
//! 音频特征**直接替换**进 embedding 序列（`<|audio_pad|>` 位置），而通用
//! `ModelForCausalLM` 只接受 token id。附带好处是不引入 candle-transformers
//! 依赖。
//!
//! 与官方一致：28 层、hidden 1024、16 头 / 8 个 KV 头、head_dim 128、
//! RMSNorm eps 1e-6、RoPE theta 1e6、SwiGLU、q/k 各有一个 per-head RMSNorm。

use candle_core::{DType, Device, Module, Tensor};
use candle_nn::{
    embedding, kv_cache::KvCache, linear_no_bias, ops::softmax_last_dim, rms_norm,
    rotary_emb::rope, Embedding, Linear, RmsNorm, VarBuilder,
};

use super::config::{TextConfig, WeightLayout};
use crate::InferError;

/// RoPE 表的最大长度。ASR 的 prompt 最长约 400 token，生成上限 512，
/// 4096 足够，且只占 2 MB。
const MAX_ROPE_POS: usize = 4096;

pub struct Decoder {
    embed: Embedding,
    layers: Vec<DecLayer>,
    norm: RmsNorm,
    lm_head: Linear,
    cos: Tensor,
    sin: Tensor,
    cfg: TextConfig,
}

struct DecLayer {
    attn: DecAttn,
    mlp: DecMlp,
    ln1: RmsNorm,
    ln2: RmsNorm,
}

struct DecAttn {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    o_proj: Linear,
    q_norm: RmsNorm,
    k_norm: RmsNorm,
    cache: KvCache,
    heads: usize,
    kv_heads: usize,
    groups: usize,
    head_dim: usize,
    /// 注意力输出展平后的维度 = heads * head_dim（Qwen3 里它不等于 hidden_size）。
    attn_dim: usize,
}

struct DecMlp {
    gate: Linear,
    up: Linear,
    down: Linear,
}

impl Decoder {
    pub fn load(
        vb: &VarBuilder,
        cfg: &TextConfig,
        layout: WeightLayout,
        dev: &Device,
        dtype: DType,
    ) -> Result<Self, InferError> {
        let vb_text = vb.pp(layout.text_model());

        let embed = embedding(cfg.vocab_size, cfg.hidden_size, vb_text.pp("embed_tokens"))?;

        let vb_layers = vb_text.pp("layers");
        let mut layers = Vec::with_capacity(cfg.layers);
        for i in 0..cfg.layers {
            layers.push(DecLayer::new(cfg, vb_layers.pp(i))?);
        }
        let norm = rms_norm(cfg.hidden_size, cfg.rms_norm_eps, vb_text.pp("norm"))?;

        // 权重里有独立的 lm_head；缺失时按 tie_word_embeddings 复用嵌入矩阵。
        let lm_head = if vb.contains_tensor(&format!("{}.weight", layout.lm_head())) {
            linear_no_bias(cfg.hidden_size, cfg.vocab_size, vb.pp(layout.lm_head()))?
        } else {
            Linear::new(embed.embeddings().clone(), None)
        };

        let (cos, sin) = rope_tables(cfg, dev, dtype)?;

        Ok(Self {
            embed,
            layers,
            norm,
            lm_head,
            cos,
            sin,
            cfg: cfg.clone(),
        })
    }

    pub fn cfg(&self) -> &TextConfig {
        &self.cfg
    }

    /// `[b, seq]` → `[b, seq, hidden]`
    pub fn embed(&self, ids: &Tensor) -> Result<Tensor, InferError> {
        Ok(self.embed.forward(ids)?)
    }

    /// 预填：整段 embedding 一次前向，返回最后一个位置的 logits `[1, vocab]`。
    ///
    /// `capacity` 是 KV cache 预留的总长度（prompt + 预期生成），预分配后每步
    /// 只用 `slice_set` 写入，避免 `cat` 在 Metal 上每 token 复制整段 cache。
    pub fn prefill(&mut self, embeds: &Tensor, capacity: usize) -> Result<Tensor, InferError> {
        for layer in &mut self.layers {
            layer.attn.cache = KvCache::new(2, capacity);
        }
        self.forward_embeds(embeds, 0)
    }

    /// 自回归一步：喂一个 token，返回下一个 token（贪心）。
    pub fn step(&mut self, token: u32, offset: usize) -> Result<u32, InferError> {
        let dev = self.embed.embeddings().device();
        let ids = Tensor::from_vec(vec![token], (1, 1), dev)?;
        let embeds = self.embed.forward(&ids)?;
        let logits = self.forward_embeds(&embeds, offset)?;
        let next = logits.squeeze(0)?.argmax(0)?.to_scalar::<u32>()?;
        Ok(next)
    }

    /// 对 embedding 序列做前向，返回最后一格的 logits。
    ///
    /// `offset` 是 KV cache 里已有的 token 数（prefill 传 0）。
    pub fn forward_embeds(&mut self, embeds: &Tensor, offset: usize) -> Result<Tensor, InferError> {
        let (b, l, _) = embeds.dims3()?;
        if offset + l > MAX_ROPE_POS {
            return Err(InferError::Config(format!(
                "序列长度 {} 超过 RoPE 表上限 {MAX_ROPE_POS}",
                offset + l
            )));
        }
        let mask = if l == 1 {
            None
        } else {
            Some(causal_mask(b, l, offset, embeds.device())?.to_dtype(embeds.dtype())?)
        };

        let cos = self.cos.narrow(0, offset, l)?.contiguous()?;
        let sin = self.sin.narrow(0, offset, l)?.contiguous()?;

        let mut h = embeds.clone();
        for layer in &mut self.layers {
            h = layer.forward(&h, mask.as_ref(), &cos, &sin)?;
        }
        let last = self.norm.forward(&h)?.narrow(1, l - 1, 1)?.squeeze(1)?;
        Ok(last.apply(&self.lm_head)?)
    }

    pub fn reset(&mut self) {
        for layer in &mut self.layers {
            layer.attn.cache.reset();
        }
    }

    /// KV cache 当前长度（调试用）。
    pub fn cache_len(&self) -> usize {
        self.layers
            .first()
            .map(|l| l.attn.cache.current_seq_len())
            .unwrap_or(0)
    }
}

impl DecLayer {
    fn new(cfg: &TextConfig, vb: VarBuilder) -> Result<Self, InferError> {
        Ok(Self {
            attn: DecAttn::new(cfg, vb.pp("self_attn"))?,
            mlp: DecMlp::new(cfg, vb.pp("mlp"))?,
            ln1: rms_norm(cfg.hidden_size, cfg.rms_norm_eps, vb.pp("input_layernorm"))?,
            ln2: rms_norm(
                cfg.hidden_size,
                cfg.rms_norm_eps,
                vb.pp("post_attention_layernorm"),
            )?,
        })
    }

    fn forward(
        &mut self,
        x: &Tensor,
        mask: Option<&Tensor>,
        cos: &Tensor,
        sin: &Tensor,
    ) -> Result<Tensor, InferError> {
        let h = self.ln1.forward(x)?;
        let h = self.attn.forward(&h, mask, cos, sin)?;
        let x = (x + h)?;
        let h2 = self.mlp.forward(&self.ln2.forward(&x)?)?;
        Ok((&x + h2)?)
    }
}

impl DecAttn {
    fn new(cfg: &TextConfig, vb: VarBuilder) -> Result<Self, InferError> {
        let hidden = cfg.hidden_size;
        let hd = cfg.head_dim;
        Ok(Self {
            q_proj: linear_no_bias(hidden, cfg.heads * hd, vb.pp("q_proj"))?,
            k_proj: linear_no_bias(hidden, cfg.kv_heads * hd, vb.pp("k_proj"))?,
            v_proj: linear_no_bias(hidden, cfg.kv_heads * hd, vb.pp("v_proj"))?,
            o_proj: linear_no_bias(cfg.heads * hd, hidden, vb.pp("o_proj"))?,
            q_norm: rms_norm(hd, cfg.rms_norm_eps, vb.pp("q_norm"))?,
            k_norm: rms_norm(hd, cfg.rms_norm_eps, vb.pp("k_norm"))?,
            cache: KvCache::new(2, 1024),
            heads: cfg.heads,
            kv_heads: cfg.kv_heads,
            groups: cfg.heads / cfg.kv_heads,
            head_dim: hd,
            attn_dim: cfg.heads * hd,
        })
    }

    fn forward(
        &mut self,
        x: &Tensor,
        mask: Option<&Tensor>,
        cos: &Tensor,
        sin: &Tensor,
    ) -> Result<Tensor, InferError> {
        let (b, l, _) = x.dims3()?;
        let q = self.q_proj.forward(x)?;
        let k = self.k_proj.forward(x)?;
        let v = self.v_proj.forward(x)?;

        // [b, l, h*d] → [b, h, l, d]
        let q = q
            .reshape((b, l, self.heads, self.head_dim))?
            .transpose(1, 2)?
            .contiguous()?;
        let k = k
            .reshape((b, l, self.kv_heads, self.head_dim))?
            .transpose(1, 2)?
            .contiguous()?;
        let v = v
            .reshape((b, l, self.kv_heads, self.head_dim))?
            .transpose(1, 2)?
            .contiguous()?;

        // per-head RMSNorm：摊平成 [b*h*l, d] 再变回来
        let q =
            self.q_norm
                .forward(&q.flatten(0, 2)?)?
                .reshape((b, self.heads, l, self.head_dim))?;
        let k = self.k_norm.forward(&k.flatten(0, 2)?)?.reshape((
            b,
            self.kv_heads,
            l,
            self.head_dim,
        ))?;

        let q = rope(&q, cos, sin)?;
        let k = rope(&k, cos, sin)?;

        let (k, v) = self.cache.append(&k, &v)?;

        let k = repeat_kv(&k, self.groups)?;
        let v = repeat_kv(&v, self.groups)?;

        let scale = 1.0 / (self.head_dim as f64).sqrt();
        let mut scores = (q.matmul(&k.transpose(2, 3)?.contiguous()?)? * scale)?;
        if let Some(m) = mask {
            scores = scores.broadcast_add(m)?;
        }
        let probs = softmax_last_dim(&scores)?;
        let out =
            probs
                .matmul(&v)?
                .transpose(1, 2)?
                .contiguous()?
                .reshape((b, l, self.attn_dim))?;
        Ok(out.apply(&self.o_proj)?)
    }
}

impl DecMlp {
    fn new(cfg: &TextConfig, vb: VarBuilder) -> Result<Self, InferError> {
        Ok(Self {
            gate: linear_no_bias(cfg.hidden_size, cfg.intermediate_size, vb.pp("gate_proj"))?,
            up: linear_no_bias(cfg.hidden_size, cfg.intermediate_size, vb.pp("up_proj"))?,
            down: linear_no_bias(cfg.intermediate_size, cfg.hidden_size, vb.pp("down_proj"))?,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor, InferError> {
        let gate = self.gate.forward(x)?.silu()?;
        let up = self.up.forward(x)?;
        Ok(self.down.forward(&(gate * up)?)?)
    }
}

/// GQA：把 KV 头重复到与 Q 头数相同。
fn repeat_kv(t: &Tensor, n: usize) -> Result<Tensor, InferError> {
    if n == 1 {
        return Ok(t.clone());
    }
    let (b, h, l, d) = t.dims4()?;
    Ok(t.unsqueeze(2)?
        .expand((b, h, n, l, d))?
        .reshape((b, h * n, l, d))?)
}

/// 因果掩码：`[b, 1, tgt, tgt+offset]`，可见 0，不可见 -inf。
fn causal_mask(b: usize, tgt: usize, offset: usize, dev: &Device) -> Result<Tensor, InferError> {
    let mut data = vec![0f32; b * tgt * (tgt + offset)];
    let row = tgt + offset;
    for bi in 0..b {
        for i in 0..tgt {
            for j in 0..row {
                if j > i + offset {
                    data[bi * tgt * row + i * row + j] = f32::NEG_INFINITY;
                }
            }
        }
    }
    Ok(Tensor::from_vec(data, (b, 1, tgt, row), dev)?)
}

/// `[max_pos, head_dim/2]` 的 cos/sin 表。
fn rope_tables(
    cfg: &TextConfig,
    dev: &Device,
    dtype: DType,
) -> Result<(Tensor, Tensor), InferError> {
    let half = cfg.head_dim / 2;
    let inv: Vec<f32> = (0..half)
        .map(|i| 1.0 / cfg.rope_theta.powf(2.0 * i as f64 / cfg.head_dim as f64) as f32)
        .collect();

    let mut cos = vec![0f32; MAX_ROPE_POS * half];
    let mut sin = vec![0f32; MAX_ROPE_POS * half];
    for p in 0..MAX_ROPE_POS {
        for i in 0..half {
            let f = p as f32 * inv[i];
            cos[p * half + i] = f.cos();
            sin[p * half + i] = f.sin();
        }
    }
    let shape = (MAX_ROPE_POS, half);
    Ok((
        Tensor::from_vec(cos, shape, dev)?.to_dtype(dtype)?,
        Tensor::from_vec(sin, shape, dev)?.to_dtype(dtype)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TextConfig {
        TextConfig {
            vocab_size: 64,
            hidden_size: 32,
            intermediate_size: 64,
            layers: 1,
            heads: 4,
            kv_heads: 2,
            head_dim: 8,
            rms_norm_eps: 1e-6,
            rope_theta: 1e6,
            attention_bias: false,
            tie_word_embeddings: true,
        }
    }

    #[test]
    fn rope_table_shape_and_first_row() {
        let (cos, sin) = rope_tables(&cfg(), &Device::Cpu, DType::F32).unwrap();
        assert_eq!(cos.dims(), &[MAX_ROPE_POS, 4]);
        let c = cos.to_vec2::<f32>().unwrap();
        let s = sin.to_vec2::<f32>().unwrap();
        for i in 0..4 {
            assert!((c[0][i] - 1.0).abs() < 1e-6);
            assert!(s[0][i].abs() < 1e-6);
        }
        // 位置 1 的频率递减
        assert!(s[1][0] > s[1][3]);
    }

    #[test]
    fn causal_mask_hides_future() {
        let m = causal_mask(1, 3, 0, &Device::Cpu).unwrap();
        let v = m.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        // [1,1,3,3] 展平后 3×3
        assert_eq!(v[0], 0.0);
        assert_eq!(v[1], f32::NEG_INFINITY);
        assert_eq!(v[8], 0.0);
    }

    #[test]
    fn causal_mask_with_offset() {
        let m = causal_mask(1, 1, 5, &Device::Cpu).unwrap();
        let v = m.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        // 单步解码：最后一个位置能看到全部 6 个
        assert_eq!(v.len(), 6);
        assert!(v.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn repeat_kv_expands_heads() {
        let t = Tensor::zeros((1, 2, 3, 4), DType::F32, &Device::Cpu).unwrap();
        let r = repeat_kv(&t, 2).unwrap();
        assert_eq!(r.dims(), &[1, 4, 3, 4]);
    }
}
