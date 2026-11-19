//! Qwen3-ASR 的模型配置（`config.json`）。
//!
//! 同一份权重有两种发布格式，字段位置与权重前缀都不同：
//!
//! | 格式 | `config.json` | 音频塔 | 文本模型 | 投影层 |
//! |---|---|---|---|---|
//! | `-hf`（transformers 格式） | 顶层 `audio_config` / `text_config` | `model.audio_tower` | `model.language_model` | `model.multi_modal_projector.linear_{1,2}` |
//! | 原始（ModelScope 首发） | 嵌在 `thinker_config` 里 | `thinker.audio_tower` | `thinker.model` | `thinker.audio_tower.proj{1,2}` |
//!
//! 两者张量内容一致，只是命名不同，所以配置解析和权重前缀分开处理。

use std::path::Path;

use serde_json::Value;

use crate::InferError;

/// 权重命名布局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeightLayout {
    /// transformers 格式（`-hf` 仓库）。
    Hf,
    /// ModelScope 原始格式（`thinker.` 前缀）。
    Original,
}

impl WeightLayout {
    /// 从 `config.json` 是否含 `thinker_config` 判断。
    pub fn detect(root: &Value) -> Self {
        if root.get("thinker_config").is_some() {
            Self::Original
        } else {
            Self::Hf
        }
    }

    pub fn audio_tower(self) -> &'static str {
        match self {
            Self::Hf => "model.audio_tower",
            Self::Original => "thinker.audio_tower",
        }
    }

    pub fn text_model(self) -> &'static str {
        match self {
            Self::Hf => "model.language_model",
            Self::Original => "thinker.model",
        }
    }

    pub fn lm_head(self) -> &'static str {
        match self {
            Self::Hf => "lm_head",
            Self::Original => "thinker.lm_head",
        }
    }

    /// 多模态投影层的 `(linear_1, linear_2)` 前缀。
    pub fn projector(self) -> (&'static str, &'static str) {
        match self {
            Self::Hf => (
                "model.multi_modal_projector.linear_1",
                "model.multi_modal_projector.linear_2",
            ),
            Self::Original => ("thinker.audio_tower.proj1", "thinker.audio_tower.proj2"),
        }
    }
}

/// 音频编码器（Whisper 式 conv stem + 窗口注意力 transformer）。
#[derive(Debug, Clone, PartialEq)]
pub struct AudioTowerConfig {
    pub d_model: usize,
    pub layers: usize,
    pub heads: usize,
    pub ffn_dim: usize,
    /// 投影到文本模型 hidden_size 的输出维度。
    pub output_dim: usize,
    /// conv stem 通道数（480）。
    pub downsample_hidden_size: usize,
    /// 半个 chunk 的帧数：chunk = `n_window * 2` = 100 帧 = 1 秒。
    pub n_window: usize,
    /// 推理注意力窗口（原始帧数）：800 帧 = 8 个 chunk。
    pub n_window_infer: usize,
    pub num_mel_bins: usize,
    /// 正弦位置编码表长度（= 每个 chunk 的 token 数，13）。
    pub max_position_embeddings: usize,
    pub scale_embedding: bool,
}

impl AudioTowerConfig {
    pub fn head_dim(&self) -> usize {
        self.d_model / self.heads
    }

    /// 一个 conv chunk 的原始 mel 帧数（100）。
    pub fn chunk_frames(&self) -> usize {
        self.n_window * 2
    }

    /// 一个 chunk 过完三次 stride-2 conv 后的 token 数（13）。
    pub fn tokens_per_chunk(&self) -> usize {
        conv_out_len(conv_out_len(conv_out_len(self.chunk_frames())))
    }

    /// 注意力窗口内 token 数（13 × 8 = 104）。
    pub fn window_tokens(&self) -> usize {
        self.tokens_per_chunk() * (self.n_window_infer / self.chunk_frames())
    }
}

/// 一次 `kernel=3, stride=2, padding=1` 卷积后的长度。
pub fn conv_out_len(frames: usize) -> usize {
    if frames == 0 {
        0
    } else {
        (frames - 1) / 2 + 1
    }
}

/// 文本解码器（Qwen3 0.6B / 1.7B）。
#[derive(Debug, Clone, PartialEq)]
pub struct TextConfig {
    pub vocab_size: usize,
    pub hidden_size: usize,
    pub intermediate_size: usize,
    pub layers: usize,
    pub heads: usize,
    pub kv_heads: usize,
    pub head_dim: usize,
    pub rms_norm_eps: f64,
    pub rope_theta: f64,
    pub attention_bias: bool,
    pub tie_word_embeddings: bool,
}

/// 模型完整配置。
#[derive(Debug, Clone)]
pub struct AsrConfig {
    pub audio: AudioTowerConfig,
    pub text: TextConfig,
    pub layout: WeightLayout,
    /// `<|audio_pad|>`：prompt 里被替换成音频特征的占位符。
    pub audio_token_id: u32,
    pub audio_start_token_id: u32,
    pub audio_end_token_id: u32,
    /// 生成终止符（`<|endoftext|>` 与 `<|im_end|>`）。
    pub eos_token_ids: Vec<u32>,
}

impl AsrConfig {
    /// 从模型目录读 `config.json`。
    pub fn load(dir: &Path) -> Result<Self, InferError> {
        let path = dir.join("config.json");
        let text = std::fs::read_to_string(&path).map_err(|source| InferError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let root: Value = serde_json::from_str(&text)?;
        Self::from_json(&root)
    }

    pub fn from_json(root: &Value) -> Result<Self, InferError> {
        let layout = WeightLayout::detect(root);
        let thinker = root.get("thinker_config").unwrap_or(root);

        let audio = thinker
            .get("audio_config")
            .ok_or_else(|| InferError::Config("缺少 audio_config".into()))?;
        let text = thinker
            .get("text_config")
            .ok_or_else(|| InferError::Config("缺少 text_config".into()))?;

        let audio = AudioTowerConfig {
            d_model: get_usize(audio, "d_model")?,
            layers: get_usize(audio, "encoder_layers")
                .or_else(|_| get_usize(audio, "num_hidden_layers"))?,
            heads: get_usize(audio, "encoder_attention_heads")?,
            ffn_dim: get_usize(audio, "encoder_ffn_dim")?,
            output_dim: get_usize(audio, "output_dim")?,
            downsample_hidden_size: get_usize(audio, "downsample_hidden_size")?,
            n_window: get_usize(audio, "n_window")?,
            n_window_infer: get_usize(audio, "n_window_infer")?,
            num_mel_bins: get_usize(audio, "num_mel_bins")?,
            max_position_embeddings: get_usize(audio, "max_position_embeddings").unwrap_or(13),
            scale_embedding: get_bool(audio, "scale_embedding").unwrap_or(false),
        };

        if audio.n_window == 0 || !audio.n_window_infer.is_multiple_of(audio.chunk_frames()) {
            return Err(InferError::Config(format!(
                "n_window={} / n_window_infer={} 与 chunk 不整除",
                audio.n_window, audio.n_window_infer
            )));
        }
        if !audio.d_model.is_multiple_of(audio.heads) {
            return Err(InferError::Config("d_model 不能被 heads 整除".into()));
        }

        let text = TextConfig {
            vocab_size: get_usize(text, "vocab_size")?,
            hidden_size: get_usize(text, "hidden_size")?,
            intermediate_size: get_usize(text, "intermediate_size")?,
            layers: get_usize(text, "num_hidden_layers")?,
            heads: get_usize(text, "num_attention_heads")?,
            kv_heads: get_usize(text, "num_key_value_heads")?,
            head_dim: get_usize(text, "head_dim")?,
            rms_norm_eps: get_f64(text, "rms_norm_eps").unwrap_or(1e-6),
            rope_theta: get_f64(text, "rope_theta").unwrap_or(1_000_000.0),
            attention_bias: get_bool(text, "attention_bias").unwrap_or(false),
            tie_word_embeddings: get_bool(text, "tie_word_embeddings").unwrap_or(true),
        };

        if !text.heads.is_multiple_of(text.kv_heads) {
            return Err(InferError::Config("注意力头数不能被 KV 头数整除".into()));
        }

        // 特殊 token 在顶层，两种格式都有；缺失时退回 Qwen 的固定 id。
        let top = root;
        let audio_token_id = get_u32(top, "audio_token_id").unwrap_or(151676);
        let audio_start_token_id = get_u32(top, "audio_start_token_id").unwrap_or(151669);
        let audio_end_token_id = get_u32(top, "audio_end_token_id").unwrap_or(151670);

        let eos_token_ids = match top.get("eos_token_id") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_u64())
                .map(|v| v as u32)
                .collect(),
            Some(Value::Number(n)) => n.as_u64().map(|v| vec![v as u32]).unwrap_or_default(),
            _ => Vec::new(),
        };
        let eos_token_ids = if eos_token_ids.is_empty() {
            vec![151643, 151645]
        } else {
            eos_token_ids
        };

        Ok(Self {
            audio,
            text,
            layout,
            audio_token_id,
            audio_start_token_id,
            audio_end_token_id,
            eos_token_ids,
        })
    }
}

fn get_usize(obj: &Value, key: &str) -> Result<usize, InferError> {
    obj.get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .ok_or_else(|| InferError::Config(format!("缺少字段或类型不对：{key}")))
}

fn get_u32(obj: &Value, key: &str) -> Option<u32> {
    obj.get(key).and_then(Value::as_u64).map(|v| v as u32)
}

fn get_f64(obj: &Value, key: &str) -> Option<f64> {
    obj.get(key).and_then(Value::as_f64)
}

fn get_bool(obj: &Value, key: &str) -> Option<bool> {
    obj.get(key).and_then(Value::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hf_config() -> Value {
        json!({
            "model_type": "qwen3_asr",
            "audio_config": {
                "d_model": 896, "encoder_layers": 18, "encoder_attention_heads": 14,
                "encoder_ffn_dim": 3584, "output_dim": 1024, "downsample_hidden_size": 480,
                "n_window": 50, "n_window_infer": 800, "num_mel_bins": 128,
                "max_position_embeddings": 13, "scale_embedding": false
            },
            "text_config": {
                "vocab_size": 151936, "hidden_size": 1024, "intermediate_size": 3072,
                "num_hidden_layers": 28, "num_attention_heads": 16, "num_key_value_heads": 8,
                "head_dim": 128, "rms_norm_eps": 1e-6, "rope_theta": 1e6,
                "attention_bias": false, "tie_word_embeddings": true
            },
            "audio_token_id": 151676, "audio_start_token_id": 151669,
            "audio_end_token_id": 151670, "eos_token_id": [151643, 151645]
        })
    }

    #[test]
    fn parses_hf_layout() {
        let cfg = AsrConfig::from_json(&hf_config()).unwrap();
        assert_eq!(cfg.layout, WeightLayout::Hf);
        assert_eq!(cfg.audio.tokens_per_chunk(), 13);
        assert_eq!(cfg.audio.window_tokens(), 104);
        assert_eq!(cfg.audio.head_dim(), 64);
        assert_eq!(cfg.text.head_dim, 128);
        assert_eq!(cfg.eos_token_ids, vec![151643, 151645]);
    }

    #[test]
    fn parses_original_layout() {
        let mut root = json!({ "thinker_config": {} });
        root["thinker_config"]["audio_config"] = hf_config()["audio_config"].clone();
        root["thinker_config"]["text_config"] = hf_config()["text_config"].clone();
        root["eos_token_id"] = json!([151643, 151645]);
        let cfg = AsrConfig::from_json(&root).unwrap();
        assert_eq!(cfg.layout, WeightLayout::Original);
        assert_eq!(cfg.layout.audio_tower(), "thinker.audio_tower");
        assert_eq!(cfg.layout.text_model(), "thinker.model");
    }

    #[test]
    fn layout_prefixes_differ_only_in_projection() {
        let hf = WeightLayout::Hf;
        let orig = WeightLayout::Original;
        assert_eq!(hf.projector().0, "model.multi_modal_projector.linear_1");
        assert_eq!(orig.projector().0, "thinker.audio_tower.proj1");
        assert_eq!(hf.lm_head(), "lm_head");
        assert_eq!(orig.lm_head(), "thinker.lm_head");
    }

    #[test]
    fn rejects_mismatched_window() {
        let mut root = hf_config();
        root["audio_config"]["n_window_infer"] = json!(750);
        let err = AsrConfig::from_json(&root).unwrap_err();
        assert!(matches!(err, InferError::Config(_)));
    }

    #[test]
    fn conv_lengths_match_torch_formula() {
        // 与 transformers `_get_feat_extract_output_lengths` 对齐
        assert_eq!(conv_out_len(100), 50);
        assert_eq!(conv_out_len(50), 25);
        assert_eq!(conv_out_len(25), 13);
        assert_eq!(conv_out_len(7), 4);
        assert_eq!(conv_out_len(0), 0);
        assert_eq!(conv_out_len(1), 1);
    }
}
