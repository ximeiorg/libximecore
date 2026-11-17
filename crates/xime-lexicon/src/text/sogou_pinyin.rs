//! 搜狗拼音文本词库（`.txt`，GBK）。
//!
//! 每行形如 `'ni'hao 你好`：行首 `'` 后是拼音，空格后是词。没有词频。

use crate::support::{build, decode_head, decode_strict, pinyin_code};
use crate::{CodeSystem, Format, LexiconError, ParsedLexicon, corrupt};
use encoding_rs::GBK;

pub(crate) struct SogouPinyin;

impl Format for SogouPinyin {
    fn id(&self) -> &'static str {
        "sgpy"
    }

    fn display_name(&self) -> &'static str {
        "搜狗拼音文本词库"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".txt"]
    }

    fn max_bytes(&self) -> u64 {
        32 * 1024 * 1024
    }

    fn code_system(&self) -> CodeSystem {
        CodeSystem::Pinyin
    }

    fn looks_like(&self, bytes: &[u8]) -> bool {
        let Some(text) = decode_head(bytes, GBK) else {
            return false;
        };
        let mut lines = 0usize;
        let mut matched = 0usize;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            lines += 1;
            if looks_like_row(line.trim()) {
                matched += 1;
            }
        }
        lines > 0 && matched * 10 >= lines * 6
    }

    fn parse(&self, bytes: &[u8]) -> Result<ParsedLexicon, LexiconError> {
        let text = decode_strict(bytes, GBK, usize::MAX)
            .ok_or_else(|| corrupt("不是 GBK 文本，无法按搜狗拼音文本词库解析"))?;

        let mut rows = Vec::new();
        let mut malformed = 0;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some(body) = trimmed.strip_prefix('\'') else {
                malformed += 1;
                continue;
            };
            let Some(space) = body.find(' ') else {
                malformed += 1;
                continue;
            };
            if space == 0 {
                malformed += 1;
                continue;
            }
            let code = pinyin_code(body[..space].split('\''));
            let word = body[space + 1..].trim();
            if code.is_empty() || word.is_empty() {
                malformed += 1;
                continue;
            }
            rows.push((word.to_string(), code, 0));
        }

        let built = build(&rows);
        Ok(ParsedLexicon {
            format_id: self.id(),
            format_name: self.display_name(),
            code_system: self.code_system(),
            entries: built.entries,
            skipped: built.skipped + malformed,
            duplicates: built.duplicates,
        })
    }
}

/// `^'[a-zA-Z']+\s+\S.*$`：行首单引号 + 纯字母/单引号的拼音 + 空白 + 词。
fn looks_like_row(line: &str) -> bool {
    let Some(body) = line.strip_prefix('\'') else {
        return false;
    };
    let code_end = body
        .find(|ch: char| !(ch.is_ascii_alphabetic() || ch == '\''))
        .unwrap_or(body.len());
    if code_end == 0 {
        return false;
    }
    let rest = &body[code_end..];
    if !rest.starts_with([' ', '\t']) {
        return false;
    }
    !rest.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/sgpy/sogou.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture")
    }

    #[test]
    fn parses_sogou_text_dictionary() {
        let parsed = SogouPinyin.parse(&fixture()).expect("parse");
        assert_eq!(parsed.format_id, "sgpy");
        assert_eq!(parsed.code_system, CodeSystem::Pinyin);
        assert_eq!(parsed.entries.len(), 7);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.duplicates, 0);
        assert_eq!(parsed.entries[0].word, "深蓝词库转换器");
        assert_eq!(parsed.entries[0].code, "shen lan ci ku zhuan huan qi");
        assert_eq!(parsed.entries.last().unwrap().word, "深");
        assert_eq!(parsed.entries.last().unwrap().code, "shen");
        assert!(parsed.entries.iter().all(|entry| entry.weight == 1));
    }

    #[test]
    fn detects_gbk_sogou_text() {
        assert!(SogouPinyin.looks_like(&fixture()));
    }

    #[test]
    fn row_shape() {
        assert!(looks_like_row("'ni'hao 你好"));
        assert!(!looks_like_row("你好 ni'hao"));
        assert!(!looks_like_row("'nihao"));
        assert!(!looks_like_row("'"));
    }
}
