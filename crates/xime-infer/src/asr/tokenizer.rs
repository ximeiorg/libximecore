//! Qwen3 分词器与 ASR 的 prompt / 输出约定。
//!
//! prompt 结构（与官方 chat template 一致，无 system 文本时）：
//!
//! ```text
//! <|im_start|>system\n<|im_end|>\n<|im_start|>user\n<|audio_start|>
//!   <|audio_pad|> × N
//! <|audio_end|><|im_end|>\n<|im_start|>assistant\n
//! ```
//!
//! 模型输出形如 `language Chinese<asr_text>转录文本`，直到 `<|im_end|>`。
//!
//! 注意：音频特征不是以 token 形式喂给解码器的——`<|audio_pad|>` 位置在
//! embeddings 里被替换成音频塔输出（详见 [`crate::asr::Qwen3Asr`]）。这里只
//! 负责前缀、后缀的 token id，以及把生成结果拆成「语种 + 文本」。

use std::path::Path;

use tokenizers::Tokenizer;

use crate::InferError;

pub const TOKEN_IM_START: u32 = 151644;
pub const TOKEN_IM_END: u32 = 151645;
pub const TOKEN_ENDOFTEXT: u32 = 151643;
/// `<|audio_start|>`
pub const TOKEN_AUDIO_START: u32 = 151669;
/// `<|audio_end|>`
pub const TOKEN_AUDIO_END: u32 = 151670;
/// `<|audio_pad|>`：音频特征占位符。
pub const TOKEN_AUDIO_PAD: u32 = 151676;
/// `<asr_text>`：语种前缀与正文的分隔符。
pub const TOKEN_ASR_TEXT: u32 = 151704;

/// 文本 token：`system` / 换行 / `user` / `assistant`。
const TOKEN_SYSTEM: u32 = 8948;
const TOKEN_NEWLINE: u32 = 198;
const TOKEN_USER: u32 = 872;
const TOKEN_ASSISTANT: u32 = 77091;

/// 一次请求的 prompt 两段（音频特征插在中间）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptIds {
    pub prefix: Vec<u32>,
    pub suffix: Vec<u32>,
}

pub struct AsrTokenizer {
    inner: Tokenizer,
}

impl AsrTokenizer {
    /// 优先 `tokenizer.json`（`-hf` 格式），退回 `vocab.json + merges.txt`。
    pub fn from_dir(dir: &Path) -> Result<Self, InferError> {
        let json = dir.join("tokenizer.json");
        if json.exists() {
            let inner = Tokenizer::from_file(&json)
                .map_err(|e| InferError::Tokenizer(format!("{}：{e}", json.display())))?;
            return Ok(Self { inner });
        }

        let vocab = dir.join("vocab.json");
        let merges = dir.join("merges.txt");
        if !vocab.exists() || !merges.exists() {
            return Err(InferError::MissingFile(format!(
                "{}（tokenizer.json 或 vocab.json + merges.txt）",
                dir.display()
            )));
        }

        let bpe = tokenizers::models::bpe::BPE::from_file(
            &vocab.to_string_lossy(),
            &merges.to_string_lossy(),
        )
        .build()
        .map_err(|e| InferError::Tokenizer(format!("BPE 装载失败：{e}")))?;

        let mut inner = Tokenizer::new(bpe);
        inner.with_pre_tokenizer(Some(
            tokenizers::pre_tokenizers::byte_level::ByteLevel::default().add_prefix_space(false),
        ));
        inner.with_decoder(Some(tokenizers::decoders::byte_level::ByteLevel::default()));
        Ok(Self { inner })
    }

    pub fn decode(&self, ids: &[u32]) -> Result<String, InferError> {
        self.inner
            .decode(ids, true)
            .map_err(|e| InferError::Tokenizer(format!("解码失败：{e}")))
    }

    pub fn encode(&self, text: &str) -> Result<Vec<u32>, InferError> {
        self.inner
            .encode(text, false)
            .map(|e| e.get_ids().to_vec())
            .map_err(|e| InferError::Tokenizer(format!("编码失败：{e}")))
    }

    /// 配置校验：确认分词器认识 prompt 里用到的固定 token id。
    ///
    /// 只检查 `system`/`user`/`assistant` 这三个文本 token——它们随 tokenizer
    /// 版本变化，错了会静默识别成乱码，值得在装载时挡一下。
    pub fn validate_prompt_tokens(&self) -> Result<(), InferError> {
        for (id, text) in [
            (TOKEN_SYSTEM, "system"),
            (TOKEN_USER, "user"),
            (TOKEN_ASSISTANT, "assistant"),
        ] {
            let encoded = self.encode(text)?;
            if encoded != [id] {
                return Err(InferError::Tokenizer(format!(
                    "prompt token 不一致：`{text}` 编码为 {encoded:?}，期望 [{id}]"
                )));
            }
        }
        Ok(())
    }
}

/// 构造 prompt 的两段 token。
///
/// `language` 为 `Some("Chinese")` 时，在 assistant 轮预填
/// `language Chinese<asr_text>`，强制模型直接输出该语种的正文；
/// 为 `None` 时由模型自己先判语种。
pub fn prompt_ids(audio_start: u32, audio_end: u32) -> PromptIds {
    let prefix = vec![
        TOKEN_IM_START,
        TOKEN_SYSTEM,
        TOKEN_NEWLINE,
        TOKEN_IM_END,
        TOKEN_NEWLINE,
        TOKEN_IM_START,
        TOKEN_USER,
        TOKEN_NEWLINE,
        audio_start,
    ];
    let suffix = vec![
        audio_end,
        TOKEN_IM_END,
        TOKEN_NEWLINE,
        TOKEN_IM_START,
        TOKEN_ASSISTANT,
        TOKEN_NEWLINE,
    ];
    PromptIds { prefix, suffix }
}

/// 带分词器的完整 prompt：`language` 需要编码，所以放在这里。
pub fn build_prompt(
    tok: &AsrTokenizer,
    audio_start: u32,
    audio_end: u32,
    language: Option<&str>,
) -> Result<PromptIds, InferError> {
    let mut ids = prompt_ids(audio_start, audio_end);
    if let Some(lang) = language {
        let mut prefill = tok.encode(&format!("language {lang}"))?;
        prefill.push(TOKEN_ASR_TEXT);
        ids.suffix.extend(prefill);
    }
    Ok(ids)
}

/// 模型原始输出拆成语种与正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedText {
    pub language: Option<String>,
    pub text: String,
}

/// `language Chinese<asr_text>你好` → `{ language: Some("Chinese"), text: "你好" }`。
///
/// 没有 `<asr_text>` 标记时按纯文本处理（模型没按格式输出时的兜底）。
pub fn parse_output(tok: &AsrTokenizer, ids: &[u32]) -> Result<ParsedText, InferError> {
    match ids.iter().position(|id| *id == TOKEN_ASR_TEXT) {
        Some(pos) => {
            let mut head = tok.decode(&ids[..pos])?.trim().to_string();
            let text = tok.decode(&ids[pos + 1..])?.trim().to_string();
            if let Some(rest) = head.strip_prefix("language ") {
                head = rest.trim().to_string();
            }
            let language = if head.is_empty() || head.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(head)
            };
            Ok(ParsedText { language, text })
        }
        None => Ok(ParsedText {
            language: None,
            text: tok.decode(ids)?.trim().to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_prefix_matches_chat_template() {
        let p = prompt_ids(TOKEN_AUDIO_START, TOKEN_AUDIO_END);
        assert_eq!(
            p.prefix,
            vec![
                TOKEN_IM_START,
                TOKEN_SYSTEM,
                TOKEN_NEWLINE,
                TOKEN_IM_END,
                TOKEN_NEWLINE,
                TOKEN_IM_START,
                TOKEN_USER,
                TOKEN_NEWLINE,
                TOKEN_AUDIO_START
            ]
        );
        assert_eq!(
            p.suffix,
            vec![
                TOKEN_AUDIO_END,
                TOKEN_IM_END,
                TOKEN_NEWLINE,
                TOKEN_IM_START,
                TOKEN_ASSISTANT,
                TOKEN_NEWLINE
            ]
        );
    }

    #[test]
    fn prompt_uses_config_audio_ids() {
        let p = prompt_ids(42, 43);
        assert_eq!(p.prefix.last(), Some(&42));
        assert_eq!(p.suffix.first(), Some(&43));
    }
}
