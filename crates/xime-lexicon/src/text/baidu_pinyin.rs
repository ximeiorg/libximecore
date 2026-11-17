//! 百度拼音文本词库（`.txt`，UTF-16LE）。
//!
//! 每行「词<Tab>拼音<Tab>词频」，拼音用 `'` 分音节、末尾可能带一个 `'`；
//! 只有两列的英文词条没有拼音，拼音词库用不上，跳过。

use crate::support::{build, decode_head, decode_strict, pinyin_code, weight_of_i64};
use crate::{CodeSystem, Format, LexiconError, ParsedLexicon, corrupt};
use encoding_rs::UTF_16LE;

pub(crate) struct BaiduPinyin;

impl Format for BaiduPinyin {
    fn id(&self) -> &'static str {
        "bdpy"
    }

    fn display_name(&self) -> &'static str {
        "百度拼音文本词库"
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
        let Some(text) = decode_head(bytes, UTF_16LE) else {
            return false;
        };
        let mut lines = 0usize;
        let mut matched = 0usize;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            lines += 1;
            let columns: Vec<&str> = line.split('\t').collect();
            if columns.len() >= 2
                && !columns[0].trim().is_empty()
                && !pinyin_code(columns[1].split('\'').filter(|part| !part.is_empty())).is_empty()
            {
                matched += 1;
            }
        }
        lines > 0 && matched * 10 >= lines * 6
    }

    fn parse(&self, bytes: &[u8]) -> Result<ParsedLexicon, LexiconError> {
        if !bytes.len().is_multiple_of(2) {
            return Err(corrupt("UTF-16LE 字节数不是偶数，文件可能不完整"));
        }
        let text = decode_strict(bytes, UTF_16LE, usize::MAX)
            .ok_or_else(|| corrupt("不是 UTF-16LE 文本，无法按百度拼音文本词库解析"))?;

        let mut rows = Vec::new();
        let mut malformed = 0;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let columns: Vec<&str> = line.split('\t').collect();
            if columns.len() < 2 {
                malformed += 1;
                continue;
            }
            let code = pinyin_code(columns[1].split('\'').filter(|part| !part.is_empty()));
            let word = columns[0].trim().to_string();
            if code.is_empty() || word.is_empty() {
                malformed += 1;
                continue;
            }
            let rank = columns
                .get(2)
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or(0);
            rows.push((word, code, weight_of_i64(rank)));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::libime::LibimeText;

    fn fixture(path: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/{path}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture")
    }

    #[test]
    fn parses_baidu_text_dictionary() {
        let parsed = BaiduPinyin.parse(&fixture("bdpy/baidu.txt")).expect("parse");
        assert_eq!(parsed.format_id, "bdpy");
        assert_eq!(parsed.entries.len(), 7);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.duplicates, 0);
        assert_eq!(parsed.entries[0].word, "深蓝词库转换器");
        assert_eq!(parsed.entries[0].code, "shen lan ci ku zhuan huan qi");
        assert_eq!(parsed.entries[0].weight, 1);
        assert_eq!(parsed.entries.last().unwrap().word, "深");
        assert_eq!(parsed.entries.last().unwrap().weight, 7);
    }

    #[test]
    fn detects_utf16_text_but_not_other_formats() {
        assert!(BaiduPinyin.looks_like(&fixture("bdpy/baidu.txt")));
        let libime = fixture("libimetxt/libime.txt");
        assert!(!BaiduPinyin.looks_like(&libime));
        assert!(LibimeText.looks_like(&libime));
    }
}
