//! Fun-ASR-Nano 的分词器与 prompt 约定。
//!
//! 这份实现**只服务 Fun-ASR**：与 Qwen3-ASR 的 `tokenizer.rs` 刻意不共用，哪怕词表
//! 是同一份 Qwen3 的。两族的 prompt 结构、输出格式、终止条件都不同，共享一份会让
//! 改一边必须验证另一边。
//!
//! prompt 结构（官方 `model.py` 的 `get_prompt` + `generate_chatml`，无热词、
//! `sys_prompt=True`、`do_think=True`）：
//!
//! ```text
//! <|im_start|>system\nYou are a helpful assistant.<|im_end|>\n
//! <|im_start|>user\n语音转写成中文：<|startofspeech|>!!<|endofspeech|><|im_end|>\n
//! <|im_start|>assistant\n
//! ```
//!
//! 音频那一段在官方的 `data_load_speech` 里是被**整段替换**的：正则按
//! `(<|startofspeech|>.*?<|endofspeech|>)` 切分，命中段整体变成「长度 = 编码器输出
//! 帧数」的 fake token，连两个标记 token 本身都不进序列。所以这里拼 prompt 时也只
//! 看「指令文本 + 音频 embedding + 收尾」三段，不需要占位 token。
//!
//! 模型输出是**纯文本**，不带头部标记，直到 `<|im_end|>` / `<|endoftext|>`。

use std::path::Path;

use tokenizers::Tokenizer;

use crate::InferError;

/// `<|im_start|>` / `<|im_end|>`：ChatML 边界。
pub const TOKEN_IM_START: u32 = 151644;
pub const TOKEN_IM_END: u32 = 151645;
/// `<|endoftext|>`：Qwen3 的 pad / eos，生成到它也要停。
pub const TOKEN_ENDOFTEXT: u32 = 151643;

/// ChatML 的几个文本 token。
///
/// 它们属于 Qwen3 词表的固定位置，[`AsrTokenizer::validate_prompt_tokens`] 会在装载
/// 时校验一遍——换词表版本时这几个 id 会静默错位，错了只会输出乱码。
const TOKEN_SYSTEM: u32 = 8948;
const TOKEN_NEWLINE: u32 = 198;
const TOKEN_USER: u32 = 872;
const TOKEN_ASSISTANT: u32 = 77091;

/// system 段的固定文本（官方 `generate_chatml` 写死这一句）。
const SYSTEM_TEXT: &str = "You are a helpful assistant.";

/// 一次请求的 prompt 两段：音频 embedding 插在 `prefix` 之后、`suffix` 之前。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptIds {
    pub prefix: Vec<u32>,
    pub suffix: Vec<u32>,
}

/// Fun-ASR 用的分词器（Qwen3 词表）。
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
    /// 只查 `system` / `user` / `assistant` 三个文本 token——它们随词表版本漂移，
    /// 错了不会报错，只会让模型看到一堆不相干的角色标签。
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

/// 构造 prompt 的两段 token（音频 embedding 拼在中间）。
///
/// `language` 会写进指令句：`Some("zh")` → `语音转写成中文：`；`None` → `语音转写：`
/// （让模型自己判语种，这是官方默认行为）。
pub fn build_prompt(tok: &AsrTokenizer, language: Option<&str>) -> Result<PromptIds, InferError> {
    let mut prefix = vec![TOKEN_IM_START, TOKEN_SYSTEM, TOKEN_NEWLINE];
    prefix.extend(tok.encode(SYSTEM_TEXT)?);
    prefix.extend([
        TOKEN_IM_END,
        TOKEN_NEWLINE,
        TOKEN_IM_START,
        TOKEN_USER,
        TOKEN_NEWLINE,
    ]);
    prefix.extend(tok.encode(&instruction(language))?);

    // 音频之后直接收尾：`<|endofspeech|>` 在官方实现里属于被替换的那一段，不留 token。
    let suffix = vec![
        TOKEN_IM_END,
        TOKEN_NEWLINE,
        TOKEN_IM_START,
        TOKEN_ASSISTANT,
        TOKEN_NEWLINE,
    ];
    Ok(PromptIds { prefix, suffix })
}

/// 用户指令句，与官方 `get_prompt` 一致（无热词、开启文本规整时）。
fn instruction(language: Option<&str>) -> String {
    match language {
        Some(lang) => format!("语音转写成{}：", language_name(lang)),
        None => "语音转写：".to_string(),
    }
}

/// 语种名归一化：checkpoint 的指令句里用的是中文名。
///
/// 官方示例传 `中文`，HF 侧传 `zh`，两种写法都得落到同一句话上。表外的原样回传
/// （模型的指令跟随能力允许它自己理解 `粤语`、`ko` 这类写法）。
pub fn language_name(lang: &str) -> &str {
    let lang = lang.trim();
    match lang.to_ascii_lowercase().as_str() {
        "zh" | "chinese" | "中文" => "中文",
        "en" | "english" | "英文" => "英文",
        "ja" | "japanese" | "日文" => "日文",
        "ko" | "korean" | "韩文" => "韩文",
        "yue" | "cantonese" | "粤语" => "粤语",
        _ => lang,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_matches_official_wording() {
        assert_eq!(instruction(Some("中文")), "语音转写成中文：");
        assert_eq!(instruction(Some("zh")), "语音转写成中文：");
        assert_eq!(instruction(None), "语音转写：");
        // 官方只在 `itn=False` 时追加「，不进行文本规整」，本实现始终开规整
        assert!(!instruction(Some("en")).contains("规整"));
    }

    #[test]
    fn language_names_map_to_instruction_names() {
        for (input, expect) in [
            ("zh", "中文"),
            ("Chinese", "中文"),
            (" 中文 ", "中文"),
            ("en", "英文"),
            ("ja", "日文"),
            ("yue", "粤语"),
            // 表外的写法原样回传，交给模型自己理解
            ("thai", "thai"),
        ] {
            assert_eq!(language_name(input), expect);
        }
    }

    #[test]
    fn system_prompt_is_the_official_sentence() {
        // 官方 `generate_chatml` 的 system 段是一整句，不是空段
        assert_eq!(SYSTEM_TEXT, "You are a helpful assistant.");
    }
}
