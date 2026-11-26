//! Whisper 的模型配置（`config.json` + `generation_config.json`）。
//!
//! 一次装载要读两个文件，职责分得很清楚：
//!
//! | 文件 | 提供什么 | 缺失时 |
//! | --- | --- | --- |
//! | `config.json` | 结构超参：层数、宽度、mel 维度、位置表长度、起止 token | 直接报错 |
//! | `generation_config.json` | 解码约定：语种表、task、`<|notimestamps|>`、抑制表 | 允许缺失，按 tokenizer 推 |
//!
//! **不要硬编码特殊 token id**。whisper 各代的 id 不同：large-v3 因为多了一个语种，
//! `<|translate|>` 从 50358 挪到 50359、`<|transcribe|>` 从 50359 挪到 50360、
//! `<|notimestamps|>` 从 50363 挪到 50364，词表也从 51865 变成 51866。写死一套
//! 会让另一代模型「能跑但输出全错」——这种错误最难查，所以语种/task/时间戳
//! token 全部在装载时从文件里解析。
//!
//! 两代模型的超参对照（本 crate 实测过的两个）：
//!
//! | | small | large-v3-turbo |
//! | --- | --- | --- |
//! | `d_model` / 注意力头 | 768 / 12 | 1280 / 20 |
//! | 编码器 / 解码器层数 | 12 / 12 | 32 / 4 |
//! | `num_mel_bins` | 80 | 128 |
//! | 词表 / 语种数 | 51865 / 99 | 51866 / 100 |

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::InferError;

/// 前馈激活函数。Whisper 官方是 `gelu`（erf 精确式，不是 tanh 近似）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// `0.5·x·(1 + erf(x/√2))`——`config.json` 里的 `gelu`。
    Gelu,
    /// tanh 近似（`gelu_new`）。Whisper 不用，留作兼容。
    GeluTanh,
    Silu,
}

impl Activation {
    fn parse(name: &str) -> Result<Self, InferError> {
        match name {
            "gelu" => Ok(Self::Gelu),
            "gelu_new" | "gelu_pytorch_tanh" => Ok(Self::GeluTanh),
            "silu" | "swish" => Ok(Self::Silu),
            other => Err(InferError::Config(format!(
                "不支持的激活函数 `{other}`（whisper 只用 gelu）"
            ))),
        }
    }
}

/// 结构超参（`config.json`）。
#[derive(Debug, Clone, PartialEq)]
pub struct WhisperConfig {
    pub d_model: usize,
    pub encoder_layers: usize,
    pub decoder_layers: usize,
    pub encoder_heads: usize,
    pub decoder_heads: usize,
    pub encoder_ffn_dim: usize,
    pub decoder_ffn_dim: usize,
    pub vocab_size: usize,
    pub num_mel_bins: usize,
    /// 编码器位置表长度（固定 1500 = 30 s / 20 ms）。
    pub max_source_positions: usize,
    /// 解码器位置表长度（固定 448，也是单次生成的硬上限）。
    pub max_target_positions: usize,
    pub activation: Activation,
    pub scale_embedding: bool,
    /// 解码器起始 token，即 `<|startoftranscript|>`。
    pub decoder_start_token_id: u32,
    pub eos_token_id: u32,
    pub pad_token_id: u32,
    /// 解码约定；`generation_config.json` 缺失时为空表。
    pub generation: GenerationConfig,
}

/// 生成约定（`generation_config.json`）。
///
/// 只收本 crate 用得到的字段：
///
/// - `suppress_tokens`：每一步都压掉的 id（标点组合、特殊标记等）；
/// - `begin_suppress_tokens`：**第一步**压掉的 id（空白 + eot 的形态）；
/// - `lang_to_id` / `task_to_id`：语种与转写/翻译的 token；
/// - `no_timestamps_token_id`：`<|notimestamps|>`。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenerationConfig {
    pub suppress_tokens: Vec<u32>,
    pub begin_suppress_tokens: Vec<u32>,
    /// `<|notimestamps|>`；`None` 表示这份权重不支持关时间戳。
    pub no_timestamps_token_id: Option<u32>,
    /// `<|startofprev|>`（上一段文本条件）。
    pub prev_sot_token_id: Option<u32>,
    /// 语种代码（`zh` / `en` / `yue` …）→ token id。
    pub lang_to_id: BTreeMap<String, u32>,
    /// `transcribe` / `translate` → token id。
    pub task_to_id: BTreeMap<String, u32>,
}

impl WhisperConfig {
    /// 从模型目录读 `config.json`，再叠上 `generation_config.json`（可选）。
    pub fn load(dir: &Path) -> Result<Self, InferError> {
        let path = dir.join("config.json");
        let text = std::fs::read_to_string(&path).map_err(|source| InferError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let root: Value = serde_json::from_str(&text)?;
        let mut cfg = Self::from_json(&root)?;

        let gen_path = dir.join("generation_config.json");
        if gen_path.exists() {
            let text = std::fs::read_to_string(&gen_path).map_err(|source| InferError::Io {
                path: gen_path.display().to_string(),
                source,
            })?;
            let gen: Value = serde_json::from_str(&text)?;
            cfg.generation = GenerationConfig::from_json(&gen);
        }
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn from_json(root: &Value) -> Result<Self, InferError> {
        let model_type = root.get("model_type").and_then(Value::as_str);
        if let Some(t) = model_type {
            if t != "whisper" {
                return Err(InferError::Config(format!(
                    "config.json 的 model_type 是 `{t}`，不是 whisper"
                )));
            }
        }

        let cfg = Self {
            d_model: get_usize(root, "d_model")?,
            encoder_layers: get_usize(root, "encoder_layers")?,
            decoder_layers: get_usize(root, "decoder_layers")?,
            encoder_heads: get_usize(root, "encoder_attention_heads")?,
            decoder_heads: get_usize(root, "decoder_attention_heads")?,
            encoder_ffn_dim: get_usize(root, "encoder_ffn_dim")?,
            decoder_ffn_dim: get_usize(root, "decoder_ffn_dim")?,
            vocab_size: get_usize(root, "vocab_size")?,
            num_mel_bins: get_usize(root, "num_mel_bins")?,
            max_source_positions: get_usize(root, "max_source_positions")?,
            max_target_positions: get_usize(root, "max_target_positions")?,
            activation: Activation::parse(
                root.get("activation_function")
                    .and_then(Value::as_str)
                    .unwrap_or("gelu"),
            )?,
            scale_embedding: get_bool(root, "scale_embedding").unwrap_or(false),
            decoder_start_token_id: get_u32(root, "decoder_start_token_id").unwrap_or(50_258),
            eos_token_id: get_u32(root, "eos_token_id").unwrap_or(50_257),
            pad_token_id: get_u32(root, "pad_token_id").unwrap_or(50_257),
            generation: GenerationConfig::default(),
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), InferError> {
        if self.d_model == 0 || !self.d_model.is_multiple_of(self.encoder_heads) {
            return Err(InferError::Config(format!(
                "d_model={} 不能被编码器头数 {} 整除",
                self.d_model, self.encoder_heads
            )));
        }
        if !self.d_model.is_multiple_of(self.decoder_heads) {
            return Err(InferError::Config(format!(
                "d_model={} 不能被解码器头数 {} 整除",
                self.d_model, self.decoder_heads
            )));
        }
        if self.num_mel_bins != 80 && self.num_mel_bins != 128 {
            return Err(InferError::Config(format!(
                "num_mel_bins={} 不是 whisper 的 80 或 128",
                self.num_mel_bins
            )));
        }
        if self.max_source_positions == 0 || self.max_target_positions == 0 {
            return Err(InferError::Config("位置表长度为 0".into()));
        }
        if self.decoder_start_token_id as usize >= self.vocab_size {
            return Err(InferError::Config(format!(
                "decoder_start_token_id {} 超出词表 {}",
                self.decoder_start_token_id, self.vocab_size
            )));
        }
        Ok(())
    }

    pub fn encoder_head_dim(&self) -> usize {
        self.d_model / self.encoder_heads
    }

    pub fn decoder_head_dim(&self) -> usize {
        self.d_model / self.decoder_heads
    }

    /// 编码器位置数：30 s 音频按 20 ms 一步就是 1500。
    pub fn encoder_positions(&self) -> usize {
        self.max_source_positions
    }

    /// 是否是多语种模型（词表里有语种 token）。
    pub fn is_multilingual(&self) -> bool {
        !self.generation.lang_to_id.is_empty()
    }
}

impl GenerationConfig {
    pub fn from_json(root: &Value) -> Self {
        Self {
            suppress_tokens: get_u32_list(root, "suppress_tokens"),
            begin_suppress_tokens: get_u32_list(root, "begin_suppress_tokens"),
            no_timestamps_token_id: get_u32(root, "no_timestamps_token_id"),
            prev_sot_token_id: get_u32(root, "prev_sot_token_id"),
            lang_to_id: language_map(root.get("lang_to_id")),
            task_to_id: string_map(root.get("task_to_id")),
        }
    }

    /// 把调用方给的语种写法解析成 whisper 的语种代码（`zh` / `en` …）。
    ///
    /// 接受 ISO 代码（`zh`、`zh-CN` 取前半）、whisper 的 token 写法
    /// （`<|zh|>`）和常见英文名（`Chinese`）。返回的是**词表里真实存在的代码**，
    /// 不在表里就是 `None`（调用方应报错，而不是拿一个不存在的 token 去解码）。
    pub fn resolve_language(&self, query: &str) -> Option<String> {
        let raw = query.trim();
        if raw.is_empty() {
            return None;
        }
        let stripped = raw.trim_start_matches("<|").trim_end_matches("|>").trim();
        let lowered = stripped.to_ascii_lowercase();
        let primary = lowered.split(['-', '_']).next().unwrap_or(&lowered);

        // 1) 直接命中代码（zh / en / yue）
        if self.lang_to_id.contains_key(primary) {
            return Some(primary.to_string());
        }
        // 2) 英文名 / 中文名 → 代码
        let alias = language_alias(primary);
        if let Some(code) = alias {
            if self.lang_to_id.contains_key(code) {
                return Some(code.to_string());
            }
        }
        // 3) 最后按代码前缀兜底（`zh-Hans` 之类）
        for code in self.lang_to_id.keys() {
            if primary.starts_with(code.as_str()) {
                return Some(code.clone());
            }
        }
        None
    }

    /// `zh` → `<|zh|>` 的 token id。
    pub fn language_token(&self, code: &str) -> Option<u32> {
        self.lang_to_id.get(code).copied()
    }

    /// task token：`"transcribe"` / `"translate"`。
    pub fn task_token(&self, task: &str) -> Option<u32> {
        self.task_to_id.get(task).copied()
    }
}

/// 常见语言的英文名 / 中文名 → ISO 代码。只覆盖用户真会输入的写法，
/// 其余走 token 代码本身。
fn language_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "english" | "英语" | "英文" => "en",
        "chinese" | "mandarin" | "中文" | "汉语" | "普通话" => "zh",
        "cantonese" | "粤语" => "yue",
        "japanese" | "日语" | "日文" => "ja",
        "korean" | "韩语" | "韩文" => "ko",
        "german" | "德语" => "de",
        "french" | "法语" => "fr",
        "spanish" | "西班牙语" => "es",
        "russian" | "俄语" => "ru",
        "portuguese" | "葡萄牙语" => "pt",
        "italian" | "意大利语" => "it",
        "arabic" | "阿拉伯语" => "ar",
        "hindi" | "印地语" => "hi",
        "thai" | "泰语" => "th",
        "vietnamese" | "越南语" => "vi",
        "indonesian" | "印尼语" => "id",
        "turkish" | "土耳其语" => "tr",
        "ukrainian" | "乌克兰语" => "uk",
        "dutch" | "荷兰语" => "nl",
        "polish" | "波兰语" => "pl",
        "tamil" | "泰米尔语" => "ta",
        "malay" | "马来语" => "ms",
        _ => return None,
    })
}

/// `<|zh|>` → `("zh", 50260)`。
fn language_map(value: Option<&Value>) -> BTreeMap<String, u32> {
    let Some(Value::Object(map)) = value else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (key, id) in map {
        let Some(id) = id.as_u64() else { continue };
        let code = key
            .trim_start_matches("<|")
            .trim_end_matches("|>")
            .trim()
            .to_string();
        if !code.is_empty() {
            out.insert(code, id as u32);
        }
    }
    out
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, u32> {
    let Some(Value::Object(map)) = value else {
        return BTreeMap::new();
    };
    map.iter()
        .filter_map(|(k, v)| v.as_u64().map(|v| (k.clone(), v as u32)))
        .collect()
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

fn get_u32_list(obj: &Value, key: &str) -> Vec<u32> {
    obj.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_u64)
                .map(|v| v as u32)
                .collect()
        })
        .unwrap_or_default()
}

fn get_bool(obj: &Value, key: &str) -> Option<bool> {
    obj.get(key).and_then(Value::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// whisper-large-v3-turbo 的真实 `config.json` 摘要。
    fn turbo() -> Value {
        json!({
            "model_type": "whisper", "d_model": 1280,
            "encoder_layers": 32, "decoder_layers": 4,
            "encoder_attention_heads": 20, "decoder_attention_heads": 20,
            "encoder_ffn_dim": 5120, "decoder_ffn_dim": 5120, "vocab_size": 51866,
            "num_mel_bins": 128, "max_source_positions": 1500,
            "max_target_positions": 448, "activation_function": "gelu",
            "decoder_start_token_id": 50258, "eos_token_id": 50257,
            "pad_token_id": 50257, "scale_embedding": false
        })
    }

    fn turbo_generation() -> Value {
        json!({
            "begin_suppress_tokens": [220, 50257],
            "no_timestamps_token_id": 50364,
            "prev_sot_token_id": 50362,
            "suppress_tokens": [1, 2, 7],
            "lang_to_id": {"<|en|>": 50259, "<|zh|>": 50260, "<|yue|>": 50358},
            "task_to_id": {"transcribe": 50360, "translate": 50359}
        })
    }

    #[test]
    fn parses_turbo_config() {
        let mut root = turbo();
        let mut cfg = WhisperConfig::from_json(&root).unwrap();
        cfg.generation = GenerationConfig::from_json(&turbo_generation());

        assert_eq!(cfg.encoder_head_dim(), 64);
        assert_eq!(cfg.decoder_head_dim(), 64);
        assert_eq!(cfg.encoder_positions(), 1500);
        assert!(cfg.is_multilingual());
        assert_eq!(cfg.activation, Activation::Gelu);
        assert_eq!(cfg.generation.no_timestamps_token_id, Some(50_364));
        assert_eq!(cfg.generation.task_token("transcribe"), Some(50_360));
        assert_eq!(cfg.generation.language_token("zh"), Some(50_260));

        // 语种写法要兼容 ISO 代码、token 写法与英文名
        assert_eq!(cfg.generation.resolve_language("zh").as_deref(), Some("zh"));
        assert_eq!(
            cfg.generation.resolve_language("<|zh|>").as_deref(),
            Some("zh")
        );
        assert_eq!(
            cfg.generation.resolve_language("Chinese").as_deref(),
            Some("zh")
        );
        assert_eq!(
            cfg.generation.resolve_language("zh-Hans").as_deref(),
            Some("zh")
        );
        assert_eq!(cfg.generation.resolve_language("klingon"), None);

        root["model_type"] = json!("foo");
        assert!(WhisperConfig::from_json(&root).is_err());
    }

    #[test]
    fn small_and_turbo_special_ids_differ() {
        // 两代模型的 id 差 1：装载时从文件读，不能写死。
        let small = GenerationConfig::from_json(&json!({
            "no_timestamps_token_id": 50363,
            "lang_to_id": {"<|en|>": 50259},
            "task_to_id": {"transcribe": 50359}
        }));
        let turbo = GenerationConfig::from_json(&turbo_generation());
        assert_ne!(small.no_timestamps_token_id, turbo.no_timestamps_token_id);
        assert_ne!(
            small.task_token("transcribe"),
            turbo.task_token("transcribe")
        );
    }

    #[test]
    fn rejects_inconsistent_dimensions() {
        let mut root = turbo();
        root["encoder_attention_heads"] = json!(13);
        assert!(WhisperConfig::from_json(&root).is_err());

        let mut root = turbo();
        root["num_mel_bins"] = json!(96);
        assert!(WhisperConfig::from_json(&root).is_err());

        let mut root = turbo();
        root["decoder_start_token_id"] = json!(60000);
        assert!(WhisperConfig::from_json(&root).is_err());

        let mut root = turbo();
        root["activation_function"] = json!("relu");
        assert!(WhisperConfig::from_json(&root).is_err());
    }

    /// `load` 的职责是把两个文件合并；没有权重也要能跑（装载权重是上层的事）。
    #[test]
    fn load_merges_generation_config() {
        let dir = std::env::temp_dir().join(format!(
            "xime_whisper_cfg_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), turbo().to_string()).unwrap();
        std::fs::write(
            dir.join("generation_config.json"),
            turbo_generation().to_string(),
        )
        .unwrap();

        let cfg = WhisperConfig::load(&dir).unwrap();
        assert!(cfg.is_multilingual());
        assert_eq!(cfg.generation.no_timestamps_token_id, Some(50_364));
        assert_eq!(
            cfg.generation.resolve_language("Chinese").as_deref(),
            Some("zh")
        );
        let _ = std::fs::remove_dir_all(&dir);

        // 缺 generation_config 时按单语模型处理，不 panic
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), turbo().to_string()).unwrap();
        let cfg = WhisperConfig::load(&dir).unwrap();
        assert!(!cfg.is_multilingual());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_generation_config_is_allowed() {
        let cfg = WhisperConfig::from_json(&turbo()).unwrap();
        assert!(!cfg.is_multilingual());
        assert_eq!(cfg.generation.no_timestamps_token_id, None);
        assert!(cfg.generation.suppress_tokens.is_empty());
    }
}
