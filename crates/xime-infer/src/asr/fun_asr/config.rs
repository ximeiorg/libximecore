//! Fun-ASR-Nano 的配置（`config.json` + `processor_config.json`）。
//!
//! `config.json` 是三段并排的结构（transformers 的 `fun_asr_nano` 布局）：
//!
//! ```text
//! audio_config   ← 音频编码器（70 层 SAN-M）
//! adaptor_config ← 投影后的 2 层 transformer
//! text_config    ← Qwen3-0.6B
//! audio_token_id ← <|object_ref_start|>
//! ```
//!
//! ModelScope 上原始仓库的 `config.yaml` 是 FunASR 格式，字段名不同（`audio_encoder_conf`
//! / `audio_adaptor_conf` / `llm_conf`），本 crate 不认——请用
//! `tools/convert_fun_asr_nano.py` 转成 safetensors 时一并生成上面这份 `config.json`。

use std::path::Path;

use serde_json::Value;

use crate::InferError;

/// 音频编码器（SenseVoice SAN-M）。
#[derive(Debug, Clone, PartialEq)]
pub struct AudioEncoderConfig {
    /// mel 通道数（80）。
    pub num_mel_bins: usize,
    /// LFR 叠帧数（7）。输入维度 = `num_mel_bins * num_stacked_frames`。
    pub num_stacked_frames: usize,
    pub hidden_size: usize,
    pub attention_heads: usize,
    pub intermediate_size: usize,
    /// 总层数（50 转写层 + 20 时间戳层 = 70）。
    pub num_hidden_layers: usize,
    /// 末尾用于时间戳预测的层数（20）。这些层同样参与编码，不能跳过。
    pub num_timestamp_prediction_layers: usize,
    pub layer_norm_eps: f64,
    /// FSMN 深度卷积核（11）。
    pub fsmn_kernel_size: usize,
    /// 正弦位置编码表长度（2049）。
    pub max_position_embeddings: usize,
}

impl AudioEncoderConfig {
    /// 编码器输入维度（560）。
    pub fn input_size(&self) -> usize {
        self.num_mel_bins * self.num_stacked_frames
    }
}

/// 投影适配层。
#[derive(Debug, Clone, PartialEq)]
pub struct AdaptorConfig {
    /// 输出维度，必须等于文本模型 hidden（1024）。
    pub hidden_size: usize,
    pub attention_heads: usize,
    /// FFN 隐层（256，只有 hidden 的 1/4）。
    pub intermediate_size: usize,
    pub num_hidden_layers: usize,
    pub layer_norm_eps: f64,
    /// `linear_1` 的输出维度（2048）。
    pub projector_hidden_size: usize,
}

/// 文本解码器（Qwen3-0.6B）。
///
/// 结构与 Qwen3-ASR 那边同构，但**这里单独定义一份**：两族的字段来源与缺省值可以
/// 各自演进（比如 Fun-ASR 这边要多认 `llm_conf` 的写法），复用会互相牵制。
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

impl TextConfig {
    /// 解析 `config.json` 里的 `text_config` 片段。
    ///
    /// `rope_theta` 有两种写法：transformers 4.x 放顶层，5.x 收进
    /// `rope_parameters.rope_theta`，两种都认。
    pub fn from_json(text: &Value) -> Result<Self, InferError> {
        let cfg = Self {
            vocab_size: get_usize(text, "vocab_size")?,
            hidden_size: get_usize(text, "hidden_size")?,
            intermediate_size: get_usize(text, "intermediate_size")?,
            layers: get_usize(text, "num_hidden_layers")?,
            heads: get_usize(text, "num_attention_heads")?,
            kv_heads: get_usize(text, "num_key_value_heads")?,
            head_dim: get_usize(text, "head_dim")?,
            rms_norm_eps: get_f64(text, "rms_norm_eps").unwrap_or(1e-6),
            rope_theta: get_f64(text, "rope_theta")
                .or_else(|| {
                    text.get("rope_parameters")
                        .and_then(|p| get_f64(p, "rope_theta"))
                })
                .unwrap_or(1_000_000.0),
            attention_bias: get_bool(text, "attention_bias").unwrap_or(false),
            tie_word_embeddings: get_bool(text, "tie_word_embeddings").unwrap_or(true),
        };

        if !cfg.heads.is_multiple_of(cfg.kv_heads) {
            return Err(InferError::Config("注意力头数不能被 KV 头数整除".into()));
        }
        Ok(cfg)
    }
}

/// 前处理参数（`processor_config.json` 里的 `feature_extractor`）。
#[derive(Debug, Clone, PartialEq)]
pub struct FrontendConfig {
    pub sample_rate: u32,
    pub frame_length_ms: usize,
    pub frame_shift_ms: usize,
    /// LFR 叠帧数。
    pub lfr_m: usize,
    /// LFR 跳帧数。
    pub lfr_n: usize,
}

impl FrontendConfig {
    /// 缺省值即官方 checkpoint 的取值。
    pub fn new(sample_rate: u32, lfr_m: usize) -> Self {
        Self {
            sample_rate,
            frame_length_ms: 25,
            frame_shift_ms: 10,
            lfr_m,
            lfr_n: 6,
        }
    }

    /// 读 `processor_config.json`；文件不存在时用缺省值（`lfr_m` 仍来自 `config.json`）。
    pub fn load(dir: &Path, sample_rate: u32, lfr_m: usize) -> Result<Self, InferError> {
        let mut cfg = Self::new(sample_rate, lfr_m);
        let path = dir.join("processor_config.json");
        if !path.exists() {
            return Ok(cfg);
        }
        let text = std::fs::read_to_string(&path).map_err(|source| InferError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let root: Value = serde_json::from_str(&text)?;
        let Some(fe) = root.get("feature_extractor") else {
            return Ok(cfg);
        };
        if let Some(v) = fe.get("sampling_rate").and_then(Value::as_u64) {
            cfg.sample_rate = v as u32;
        }
        if let Some(v) = fe.get("frame_length").and_then(Value::as_u64) {
            cfg.frame_length_ms = v as usize;
        }
        if let Some(v) = fe.get("frame_shift").and_then(Value::as_u64) {
            cfg.frame_shift_ms = v as usize;
        }
        if let Some(v) = fe.get("num_frames_lfr").and_then(Value::as_u64) {
            cfg.lfr_m = v as usize;
        }
        if let Some(v) = fe.get("stride_lfr").and_then(Value::as_u64) {
            cfg.lfr_n = v as usize;
        }
        Ok(cfg)
    }
}

/// 模型完整配置。
#[derive(Debug, Clone)]
pub struct FunAsrConfig {
    pub audio: AudioEncoderConfig,
    pub adaptor: AdaptorConfig,
    pub text: TextConfig,
    pub frontend: FrontendConfig,
    /// `<|object_ref_start|>`：prompt 里音频的占位符。
    ///
    /// 本实现直接按 embedding 拼接，不依赖这个 id；留作装载自检与对照。
    pub audio_token_id: u32,
    /// 生成终止符（`<|im_end|>` 与 `<|endoftext|>`）。
    pub eos_token_ids: Vec<u32>,
}

impl FunAsrConfig {
    pub fn load(dir: &Path) -> Result<Self, InferError> {
        let path = dir.join("config.json");
        let text = std::fs::read_to_string(&path).map_err(|source| InferError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let root: Value = serde_json::from_str(&text)?;
        Self::from_json(&root, dir)
    }

    pub fn from_json(root: &Value, dir: &Path) -> Result<Self, InferError> {
        let audio = root
            .get("audio_config")
            .ok_or_else(|| InferError::Config("缺少 audio_config".into()))?;
        let adaptor = root
            .get("adaptor_config")
            .ok_or_else(|| InferError::Config("缺少 adaptor_config（不是 Fun-ASR-Nano）".into()))?;
        let text_json = root
            .get("text_config")
            .ok_or_else(|| InferError::Config("缺少 text_config".into()))?;

        let audio = AudioEncoderConfig {
            num_mel_bins: get_usize(audio, "num_mel_bins")?,
            num_stacked_frames: get_usize(audio, "num_stacked_frames")
                .or_else(|_| get_usize(audio, "num_frames_lfr"))?,
            hidden_size: get_usize(audio, "hidden_size")?,
            attention_heads: get_usize(audio, "num_attention_heads")?,
            intermediate_size: get_usize(audio, "intermediate_size")?,
            num_hidden_layers: get_usize(audio, "num_hidden_layers")?,
            num_timestamp_prediction_layers: get_usize(audio, "num_timestamp_prediction_layers")
                .unwrap_or(0),
            layer_norm_eps: get_f64(audio, "layer_norm_eps").unwrap_or(1e-5),
            fsmn_kernel_size: get_usize(audio, "fsmn_kernel_size").unwrap_or(11),
            max_position_embeddings: get_usize(audio, "max_position_embeddings")?,
        };

        let adaptor = AdaptorConfig {
            hidden_size: get_usize(adaptor, "hidden_size")?,
            attention_heads: get_usize(adaptor, "num_attention_heads")?,
            intermediate_size: get_usize(adaptor, "intermediate_size")?,
            num_hidden_layers: get_usize(adaptor, "num_hidden_layers")?,
            layer_norm_eps: get_f64(adaptor, "layer_norm_eps").unwrap_or(1e-5),
            projector_hidden_size: get_usize(adaptor, "projector_hidden_size")?,
        };

        let text = TextConfig::from_json(text_json)?;

        if !audio.hidden_size.is_multiple_of(audio.attention_heads) {
            return Err(InferError::Config(format!(
                "编码器 hidden {} 不能被头数 {} 整除",
                audio.hidden_size, audio.attention_heads
            )));
        }
        if adaptor.hidden_size != text.hidden_size {
            return Err(InferError::Config(format!(
                "投影层输出 {} 与文本模型 hidden {} 不一致",
                adaptor.hidden_size, text.hidden_size
            )));
        }
        if audio.num_timestamp_prediction_layers >= audio.num_hidden_layers {
            return Err(InferError::Config(
                "时间戳层数不小于总层数，至少要有 1 层转写层".into(),
            ));
        }

        let frontend = FrontendConfig::load(dir, 16_000, audio.num_stacked_frames)?;

        let audio_token_id = root
            .get("audio_token_id")
            .and_then(Value::as_u64)
            .unwrap_or(151646) as u32;

        let mut eos = match text_json.get("eos_token_id") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_u64)
                .map(|v| v as u32)
                .collect::<Vec<_>>(),
            Some(Value::Number(n)) => n.as_u64().map(|v| vec![v as u32]).unwrap_or_default(),
            _ => Vec::new(),
        };
        if eos.is_empty() {
            // <|im_end|> 与 <|endoftext|>
            eos = vec![151645, 151643];
        } else if !eos.contains(&151643) {
            // pad 也是 <|endoftext|>，生成到它就当结束，避免 endless 重复
            eos.push(151643);
        }

        Ok(Self {
            audio,
            adaptor,
            text,
            frontend,
            audio_token_id,
            eos_token_ids: eos,
        })
    }

    /// 模型是否是 Fun-ASR-Nano（用于宿主/服务层派发）。
    pub fn is_fun_asr(root: &Value) -> bool {
        match root.get("model_type").and_then(Value::as_str) {
            Some("fun_asr_nano") => true,
            // 老版本 config.json 没有 model_type：靠 adaptor_config 区分于 Qwen3-ASR
            None => root.get("adaptor_config").is_some(),
            Some(_) => false,
        }
    }
}

fn get_usize(obj: &Value, key: &str) -> Result<usize, InferError> {
    obj.get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .ok_or_else(|| InferError::Config(format!("缺少字段或类型不对：{key}")))
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

    fn config_json() -> Value {
        json!({
            "model_type": "fun_asr_nano",
            "audio_token_id": 151646,
            "audio_config": {
                "num_mel_bins": 80, "num_stacked_frames": 7, "hidden_size": 512,
                "num_attention_heads": 4, "intermediate_size": 2048,
                "num_hidden_layers": 70, "num_timestamp_prediction_layers": 20,
                "layer_norm_eps": 1e-5, "fsmn_kernel_size": 11,
                "max_position_embeddings": 2049
            },
            "adaptor_config": {
                "hidden_size": 1024, "num_attention_heads": 8, "intermediate_size": 256,
                "num_hidden_layers": 2, "layer_norm_eps": 1e-5, "projector_hidden_size": 2048
            },
            "text_config": {
                "vocab_size": 151936, "hidden_size": 1024, "intermediate_size": 3072,
                "num_hidden_layers": 28, "num_attention_heads": 16, "num_key_value_heads": 8,
                "head_dim": 128, "rms_norm_eps": 1e-6, "attention_bias": false,
                "tie_word_embeddings": true, "eos_token_id": 151645,
                "rope_parameters": { "rope_theta": 1000000, "rope_type": "default" }
            }
        })
    }

    #[test]
    fn parses_official_config() {
        let dir = std::path::Path::new("/nonexistent/fun-asr");
        let cfg = FunAsrConfig::from_json(&config_json(), dir).unwrap();
        assert_eq!(cfg.audio.input_size(), 560);
        assert_eq!(cfg.audio.num_hidden_layers, 70);
        assert_eq!(cfg.text.hidden_size, 1024);
        // transformers 5.x 把 rope_theta 收进了 rope_parameters
        assert_eq!(cfg.text.rope_theta, 1_000_000.0);
        assert_eq!(cfg.audio_token_id, 151646);
        // eos 里补上 <|endoftext|>，避免生成到 pad 之后空转
        assert_eq!(cfg.eos_token_ids, vec![151645, 151643]);
        // 没有 processor_config.json 时用官方缺省值
        assert_eq!(cfg.frontend.lfr_m, 7);
        assert_eq!(cfg.frontend.lfr_n, 6);
        assert_eq!(cfg.frontend.frame_length_ms, 25);
    }

    #[test]
    fn rejects_adaptor_text_mismatch() {
        let mut root = config_json();
        root["adaptor_config"]["hidden_size"] = json!(768);
        let err = FunAsrConfig::from_json(&root, std::path::Path::new("/nonexistent")).unwrap_err();
        assert!(matches!(err, InferError::Config(_)));
    }

    #[test]
    fn rejects_all_timestamp_layers() {
        let mut root = config_json();
        root["audio_config"]["num_timestamp_prediction_layers"] = json!(70);
        let err = FunAsrConfig::from_json(&root, std::path::Path::new("/nonexistent")).unwrap_err();
        assert!(matches!(err, InferError::Config(_)));
    }

    #[test]
    fn detects_family_by_model_type_or_adaptor() {
        assert!(FunAsrConfig::is_fun_asr(&config_json()));

        let mut no_type = config_json();
        no_type.as_object_mut().unwrap().remove("model_type");
        assert!(FunAsrConfig::is_fun_asr(&no_type));

        // Qwen3-ASR 的 config.json 也有 audio_config，但读不到 adaptor_config
        let qwen = json!({ "model_type": "qwen3_asr", "audio_config": {}, "text_config": {} });
        assert!(!FunAsrConfig::is_fun_asr(&qwen));
    }
}
