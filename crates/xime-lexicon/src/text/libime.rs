//! Fcitx5 / libime 文本拼音词库（`.txt`）。
//!
//! 每行「词 拼音 [词频]」，拼音用 `'` 分音节；词面含空白/引号/反斜杠时用引号
//! 包裹并转义，与 libime `loadText` / `saveText` 一致。词频是浮点数，可省略。

use crate::support::{
    build, decode_head, decode_strict, looks_like_pinyin_text, pinyin_code, weight_of_f64,
};
use crate::{CodeSystem, Format, LexiconError, ParsedLexicon, corrupt};
use encoding_rs::UTF_8;

pub(crate) struct LibimeText;

impl Format for LibimeText {
    fn id(&self) -> &'static str {
        "libimetxt"
    }

    fn display_name(&self) -> &'static str {
        "Fcitx5/libime 文本词库"
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
        let Some(text) = decode_head(bytes, UTF_8) else {
            return false;
        };
        looks_like_pinyin_text(&text, &tokenize)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ParsedLexicon, LexiconError> {
        let text = decode_strict(bytes, UTF_8, usize::MAX)
            .ok_or_else(|| corrupt("不是 UTF-8 文本，无法按 libime 文本词库解析"))?;
        let mut rows = Vec::new();
        let mut malformed = 0;

        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let tokens = tokenize(line);
            if !(2..=3).contains(&tokens.len()) {
                malformed += 1;
                continue;
            }
            let code = pinyin_code(tokens[1].split('\''));
            if code.is_empty() || tokens[0].trim().is_empty() {
                malformed += 1;
                continue;
            }
            let rank = tokens
                .get(2)
                .and_then(|value| value.trim().parse::<f64>().ok())
                .unwrap_or(0.0);
            rows.push((tokens[0].clone(), code, weight_of_f64(rank)));
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

/// fcitx `escapeForValue` / `consumeMaybeEscapedValue` 的对应实现。
fn tokenize(line: &str) -> Vec<String> {
    const WHITESPACE: [char; 6] = ['\u{0c}', '\n', '\r', '\t', '\u{0b}', ' '];

    let chars: Vec<char> = line.chars().collect();
    let mut tokens = Vec::with_capacity(4);
    let mut index = 0usize;

    while index < chars.len() {
        while index < chars.len() && WHITESPACE.contains(&chars[index]) {
            index += 1;
        }
        if index >= chars.len() {
            break;
        }

        if chars[index] == '"' {
            let mut value = String::new();
            let mut escaping = false;
            let mut closing = None;
            let mut cursor = index + 1;
            while cursor < chars.len() {
                let ch = chars[cursor];
                if !escaping {
                    match ch {
                        '\\' => escaping = true,
                        '"' => {
                            closing = Some(cursor + 1);
                            break;
                        }
                        _ => value.push(ch),
                    }
                } else {
                    value.push(unescape(ch));
                    escaping = false;
                }
                cursor += 1;
            }
            if let Some(closing) = closing {
                tokens.push(value);
                index = closing;
                continue;
            }
        }

        let mut stop = index + 1;
        while stop < chars.len() && !WHITESPACE.contains(&chars[stop]) {
            stop += 1;
        }
        tokens.push(chars[index..stop].iter().collect());
        index = stop;
    }

    tokens
}

fn unescape(ch: char) -> char {
    match ch {
        'n' => '\n',
        'f' => '\u{0c}',
        'r' => '\r',
        't' => '\t',
        'v' => '\u{0b}',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/libimetxt/libime.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture")
    }

    #[test]
    fn parses_libime_text_dictionary() {
        let parsed = LibimeText.parse(&fixture()).expect("parse");
        assert_eq!(parsed.format_id, "libimetxt");
        assert_eq!(parsed.code_system, CodeSystem::Pinyin);
        assert!(!parsed.entries.is_empty());
        assert!(parsed
            .entries
            .iter()
            .all(|entry| !entry.word.is_empty() && !entry.code.is_empty()));
    }

    #[test]
    fn detects_libime_text_only() {
        assert!(LibimeText.looks_like(&fixture()));
        assert!(!LibimeText.looks_like("hello world\n".as_bytes()));
    }

    #[test]
    fn tokenizes_quoted_and_escaped_values() {
        assert_eq!(tokenize("你好 ni'hao 12"), vec!["你好", "ni'hao", "12"]);
        assert_eq!(
            tokenize("\"a b\" \"x\\\"y\" 3"),
            vec!["a b", "x\"y", "3"]
        );
        assert_eq!(tokenize("   "), Vec::<String>::new());
    }
}
