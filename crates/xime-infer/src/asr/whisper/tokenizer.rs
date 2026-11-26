//! Whisper 的分词器、prompt 约定与特殊 token 解析。
//!
//! 解码 prompt 是固定的四段（多语种模型）：
//!
//! ```text
//! <|startoftranscript|> <|语种|> <|transcribe|> <|notimestamps|>
//! ```
//!
//! 单语（`.en`）模型没有语种段；`translate` 时第三段换成 `<|translate|>`。
//! 关掉时间戳（`<|notimestamps|>`）之后，模型输出就是纯文本，遇到
//! `<|endoftext|>` 停止。
//!
//! **特殊 token 的 id 不能写死**：large-v3 因为多一个语种，`<|translate|>` 从
//! 50358 变 50359、`<|transcribe|>` 从 50359 变 50360、`<|notimestamps|>` 从
//! 50363 变 50364；small 与 turbo 混用写死的 id 会解出乱码。这里从
//! `generation_config.json` 与 `tokenizer.json` 两处取，并把两边对不上的情况
//! 在装载时报错。
//!
//! 另外，whisper 的输出 token 不是「干净文本」：首 token 常带前导空格、偶发
//! `<|xx|>` 特殊标记。解码走 byte-level BPE + `skip_special_tokens`，最后
//! `trim()`——与 transformers 的 `decode(..., skip_special_tokens=True)` 等价。

use std::collections::BTreeMap;
use std::path::Path;

use tokenizers::Tokenizer;

use super::config::WhisperConfig;
use crate::InferError;

/// prompt 里要用到的特殊 token。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecialTokens {
    /// `<|startoftranscript|>`（也等于 `config.decoder_start_token_id`）。
    pub sot: u32,
    /// `<|endoftext|>`：生成终止符。
    pub eot: u32,
    pub transcribe: Option<u32>,
    pub translate: Option<u32>,
    pub notimestamps: Option<u32>,
    /// 语种代码（`zh`）→ token id。
    pub languages: BTreeMap<String, u32>,
}

impl SpecialTokens {
    /// 从 `tokenizer.json` 取特殊 token，`generation_config.json` 里有的以它为准。
    ///
    /// 两处都有时校验一致性：不一致基本意味着模型目录被拼错了（比如 small 的
    /// 权重配了 turbo 的 generation_config），这里直接报错比事后查乱码划算。
    pub fn resolve(tok: &WhisperTokenizer, cfg: &WhisperConfig) -> Result<Self, InferError> {
        let gen = &cfg.generation;
        let sot = resolve_id(
            tok,
            "<|startoftranscript|>",
            Some(cfg.decoder_start_token_id),
        )?
        .ok_or_else(|| InferError::Tokenizer("分词器里没有 <|startoftranscript|>".into()))?;

        let eot = resolve_id(tok, "<|endoftext|>", Some(cfg.eos_token_id))?
            .ok_or_else(|| InferError::Tokenizer("分词器里没有 <|endoftext|>".into()))?;

        // 语种表：generation_config 优先；缺失时从分词器扫描 `<|xx|>` 形式的 token。
        let mut languages = gen.lang_to_id.clone();
        if languages.is_empty() {
            languages = scan_language_tokens(tok);
        }
        for (code, id) in &languages {
            if *id as usize >= cfg.vocab_size {
                return Err(InferError::Config(format!(
                    "语种 {code} 的 token {id} 超出词表 {}",
                    cfg.vocab_size
                )));
            }
            // 两个文件必须来自同一代模型：generation_config 说是 `<|zh|>`=50260，
            // tokenizer 却把它标成别的 id，解码出来就是另一种语言。
            if let Some(from_tok) = tok.token_to_id(&format!("<|{code}|>")) {
                if from_tok != *id {
                    return Err(InferError::Config(format!(
                        "语种 {code} 的 id 不一致：tokenizer.json 是 {from_tok}，\
                         generation_config 是 {id}（模型目录里的文件不是同一代）"
                    )));
                }
            }
        }

        let transcribe = gen
            .task_token("transcribe")
            .or_else(|| tok.token_to_id("<|transcribe|>"));
        let translate = gen
            .task_token("translate")
            .or_else(|| tok.token_to_id("<|translate|>"));
        let notimestamps = gen
            .no_timestamps_token_id
            .or_else(|| tok.token_to_id("<|notimestamps|>"));

        Ok(Self {
            sot,
            eot,
            transcribe,
            translate,
            notimestamps,
            languages,
        })
    }

    pub fn is_multilingual(&self) -> bool {
        !self.languages.is_empty()
    }

    pub fn language_token(&self, code: &str) -> Option<u32> {
        self.languages.get(code).copied()
    }

    /// task token：`"transcribe"`（默认）或 `"translate"`。
    pub fn task_token(&self, task: &str) -> Option<u32> {
        match task {
            "translate" => self.translate,
            _ => self.transcribe,
        }
    }

    /// 构造解码 prompt。
    ///
    /// - `language`：`None` 表示单语模型（不写语种段）；
    /// - `task`：`"transcribe"` / `"translate"`；
    /// - `without_timestamps`：是否补 `<|notimestamps|>`。
    pub fn prompt(
        &self,
        language: Option<u32>,
        task: Option<u32>,
        without_timestamps: bool,
    ) -> Vec<u32> {
        let mut ids = Vec::with_capacity(4);
        ids.push(self.sot);
        if let Some(lang) = language {
            ids.push(lang);
        }
        if let Some(task) = task {
            ids.push(task);
        }
        if without_timestamps {
            if let Some(nots) = self.notimestamps {
                ids.push(nots);
            }
        }
        ids
    }

    /// 所有「禁止出现在输出里」的 id：`<|notimestamps|>` 之后的**全部**时间戳
    /// token（`<|0.00|>` … `<|30.00|>`，0.02 s 一格）。
    ///
    /// 官方 `decoding.py` 在 `without_timestamps=True` 时就是这么做的：时间戳
    /// 必须成对出现，模型一旦开始吐时间戳就没法当纯文本用；与其在输出里过滤，
    /// 不如从第一步就压掉。
    pub fn timestamp_range(&self, vocab_size: usize) -> std::ops::Range<u32> {
        match self.notimestamps {
            Some(nots) => (nots + 1)..(vocab_size as u32),
            None => 0..0,
        }
    }
}

/// 往分词器找 `<|...|>` 形式的语种 token。
fn scan_language_tokens(tok: &WhisperTokenizer) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    for (id, text) in tok.special_vocab() {
        let inner = text
            .strip_prefix("<|")
            .and_then(|s| s.strip_suffix("|>"))
            .unwrap_or_default();
        // 语种代码是 2–3 个小写字母；`<|transcribe|>` / `<|notimestamps|>`
        // 这类更长的标记自然被排除。
        if (2..=3).contains(&inner.len()) && inner.chars().all(|c| c.is_ascii_lowercase()) {
            out.insert(inner.to_string(), id);
        }
    }
    out
}

/// 优先取 generation_config 里的 id，否则回退分词器；两边都有且不同则报错。
fn resolve_id(
    tok: &WhisperTokenizer,
    text: &str,
    from_config: Option<u32>,
) -> Result<Option<u32>, InferError> {
    let from_tok = tok.token_to_id(text);
    match (from_tok, from_config) {
        (Some(t), Some(c)) if t != c => Err(InferError::Config(format!(
            "特殊 token {text} 的 id 不一致：tokenizer.json 是 {t}，config 是 {c}\
             （模型目录里的文件不是同一代）"
        ))),
        (Some(t), _) => Ok(Some(t)),
        (None, Some(c)) => Ok(Some(c)),
        (None, None) => Ok(None),
    }
}

pub struct WhisperTokenizer {
    inner: Tokenizer,
}

impl WhisperTokenizer {
    pub fn from_dir(dir: &Path) -> Result<Self, InferError> {
        let json = dir.join("tokenizer.json");
        if !json.exists() {
            return Err(InferError::MissingFile(json.display().to_string()));
        }
        let inner = Tokenizer::from_file(&json)
            .map_err(|e| InferError::Tokenizer(format!("{}：{e}", json.display())))?;
        Ok(Self { inner })
    }

    pub fn token_to_id(&self, text: &str) -> Option<u32> {
        self.inner.token_to_id(text)
    }

    /// 词表里所有「特殊 token」的 `(id, 文本)`，用于推导语种表。
    pub fn special_vocab(&self) -> Vec<(u32, String)> {
        // tokenizers 没有直接暴露 added_tokens 的遍历接口，用
        // `get_added_tokens_decoder`（id → AddedToken）。
        self.inner
            .get_added_tokens_decoder()
            .iter()
            .filter(|(_, token)| token.special)
            .map(|(id, token)| (*id, token.content.clone()))
            .collect()
    }

    /// 解码 token → 文本（跳过特殊 token，等价 `skip_special_tokens=True`）。
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
}

/// 生成结果 → 纯文本。
///
/// 除了跳过特殊 token，还要处理两类残留：
/// - 时间戳文本（模型无视 `<|notimestamps|>` 时会出现 `<|1.20|>`）——按特殊
///   token 跳过；
/// - 首尾空白（首 token 常带前导空格）。
pub fn parse_text(tok: &WhisperTokenizer, ids: &[u32]) -> Result<String, InferError> {
    Ok(tok.decode(ids)?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_layout_matches_whisper() {
        let sp = SpecialTokens {
            sot: 50_258,
            eot: 50_257,
            transcribe: Some(50_360),
            translate: Some(50_359),
            notimestamps: Some(50_364),
            languages: BTreeMap::new(),
        };
        assert_eq!(
            sp.prompt(Some(50_260), Some(50_360), true),
            vec![50_258, 50_260, 50_360, 50_364]
        );
        // 单语模型：没有语种段
        assert_eq!(sp.prompt(None, None, true), vec![50_258, 50_364]);
        // 翻译
        assert_eq!(
            sp.prompt(Some(50_260), sp.task_token("translate"), true),
            vec![50_258, 50_260, 50_359, 50_364]
        );
        // 不关时间戳
        assert_eq!(
            sp.prompt(Some(50_260), Some(50_360), false),
            vec![50_258, 50_260, 50_360]
        );
    }

    #[test]
    fn timestamp_range_starts_after_notimestamps() {
        let sp = SpecialTokens {
            sot: 50_258,
            eot: 50_257,
            transcribe: None,
            translate: None,
            notimestamps: Some(50_364),
            languages: BTreeMap::new(),
        };
        let r = sp.timestamp_range(51_866);
        assert_eq!(r.start, 50_365);
        assert_eq!(r.end, 51_866);

        let none = SpecialTokens {
            notimestamps: None,
            ..sp
        };
        assert!(none.timestamp_range(51_866).is_empty());
    }
}
