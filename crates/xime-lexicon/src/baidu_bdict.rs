//! 百度分类词库（`.bdict`）解析。
//!
//! 二进制结构与 imewlconverter 的 `BaiduBdictImporter` 一致：
//! - `0x00`：ASCII 魔数（`biptbdsw`，部分版本为 `bdict`）；
//! - `0x40`：中文词条区起始偏移，`0x44`：词条区字节长度；
//! - 每条词条：Int32 音节数，随后每字两字节（声母索引、韵母索引，`0xFF` 表示英文字母），
//!   再跟 UTF-16LE 词。
//!
//! 词条区之后还有纯英文区与混排区（编码和词不等长），拼音方案无法直接使用，跳过。

use crate::support::DEFAULT_WEIGHT;
use crate::{CodeSystem, Entry, Format, LexiconError, ParsedLexicon, corrupt};

pub(crate) struct BaiduBdict;

const HEADER_SIZE: usize = 0x350;
const OFFSET_DATA_START: usize = 0x40;
const OFFSET_DATA_LENGTH: usize = 0x44;
const OFFSET_DATA_END: usize = 0x60;
const MAX_SYLLABLES: i32 = 1000;
const MAX_ENTRIES: usize = 1_000_000;

const INITIAL_ENGLISH: u8 = 0xFF;

const MAGICS: [&str; 2] = ["biptbdsw", "bdict"];

const INITIALS: [&str; 24] = [
    "c", "d", "b", "f", "g", "h", "ch", "j", "k", "l", "m", "n", "", "p", "q", "r", "s", "t", "sh",
    "zh", "w", "x", "y", "z",
];

const FINALS: [&str; 33] = [
    "uang", "iang", "iong", "ang", "eng", "ian", "iao", "ing", "ong", "uai", "uan", "ai", "an", "ao",
    "ei", "en", "er", "ua", "ie", "in", "iu", "ou", "ia", "ue", "ui", "un", "uo", "a", "e", "i", "o",
    "u", "v",
];

impl Format for BaiduBdict {
    fn id(&self) -> &'static str {
        "baidu-bdict"
    }

    fn display_name(&self) -> &'static str {
        "百度分类词库"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".bdict"]
    }

    fn max_bytes(&self) -> u64 {
        32 * 1024 * 1024
    }

    fn code_system(&self) -> CodeSystem {
        CodeSystem::Pinyin
    }

    fn parse(&self, bytes: &[u8]) -> Result<ParsedLexicon, LexiconError> {
        let raw = parse_binary(bytes)?;
        let converted = convert(&raw);
        Ok(ParsedLexicon {
            format_id: self.id(),
            format_name: self.display_name(),
            code_system: self.code_system(),
            entries: converted.entries,
            skipped: converted.skipped,
            duplicates: converted.duplicates,
        })
    }
}

/// 解析后的一行原始词条；`mixed` 表示含英文字母，拼音码不可用。
struct RawEntry {
    word: String,
    code: String,
    mixed: bool,
}

struct Conversion {
    entries: Vec<Entry>,
    skipped: usize,
    duplicates: usize,
}

/// 原始词条 -> Rime 词条：去空、去重、跳过含英文字母的混排词。
fn convert(raw: &[RawEntry]) -> Conversion {
    let mut seen = std::collections::HashSet::with_capacity(raw.len() * 2);
    let mut entries = Vec::with_capacity(raw.len());
    let mut skipped = 0;
    let mut duplicates = 0;

    for entry in raw {
        let word = entry.word.trim();
        let code = entry.code.trim();
        if entry.mixed || word.is_empty() || code.is_empty() {
            skipped += 1;
            continue;
        }
        if word.starts_with('#') || word.contains(['\t', '\n', '\r']) {
            skipped += 1;
            continue;
        }
        if !seen.insert((word.to_string(), code.to_string())) {
            duplicates += 1;
            continue;
        }
        entries.push(Entry {
            word: word.to_string(),
            code: code.to_string(),
            weight: DEFAULT_WEIGHT,
        });
    }

    Conversion {
        entries,
        skipped,
        duplicates,
    }
}

fn parse_binary(bytes: &[u8]) -> Result<Vec<RawEntry>, LexiconError> {
    if bytes.len() < HEADER_SIZE {
        return Err(corrupt("文件太小，不是有效的 .bdict"));
    }
    check_magic(bytes)?;

    let mut start = read_u32(bytes, OFFSET_DATA_START) as usize;
    if start < HEADER_SIZE || start >= bytes.len() {
        start = HEADER_SIZE;
    }

    let mut end = start + read_u32(bytes, OFFSET_DATA_LENGTH) as usize;
    if end <= start || end > bytes.len() {
        // 少数版本 0x44 为空，退回 0x60 记录的词条区结束位置。
        end = read_u32(bytes, OFFSET_DATA_END) as usize;
    }
    if end <= start || end > bytes.len() {
        return Err(corrupt("词条区范围异常"));
    }

    let mut raw = Vec::new();
    let mut pos = start;
    while pos + 4 <= end {
        let syllables = read_i32(bytes, pos);
        pos += 4;
        if syllables == 0 {
            break;
        }
        if !(0..=MAX_SYLLABLES).contains(&syllables) {
            return Err(corrupt(format!("词条长度异常：{syllables}")));
        }
        let syllables = syllables as usize;
        let needed = syllables * 4;
        if pos + needed > end {
            return Err(corrupt("词条数据被截断"));
        }

        let mut code = String::with_capacity(syllables * 3);
        let mut mixed = false;
        for index in 0..syllables {
            let initial = bytes[pos + index * 2];
            let final_ = bytes[pos + index * 2 + 1];
            if index > 0 {
                code.push(' ');
            }
            if initial == INITIAL_ENGLISH {
                mixed = true;
                code.push(final_ as char);
            } else if (initial as usize) < INITIALS.len() && (final_ as usize) < FINALS.len() {
                code.push_str(INITIALS[initial as usize]);
                code.push_str(FINALS[final_ as usize]);
            } else {
                return Err(corrupt(format!(
                    "拼音索引异常：{initial}/{final_}"
                )));
            }
        }
        pos += syllables * 2;

        let word = utf16le(&bytes[pos..pos + syllables * 2]);
        pos += syllables * 2;
        if word.chars().count() != syllables {
            return Err(corrupt("词条数据损坏"));
        }
        raw.push(RawEntry { word, code, mixed });
        if raw.len() > MAX_ENTRIES {
            return Err(corrupt("词条数异常"));
        }
    }
    Ok(raw)
}

fn check_magic(bytes: &[u8]) -> Result<(), LexiconError> {
    let magic = String::from_utf8_lossy(&bytes[..8]);
    if !MAGICS.iter().any(|expected| magic.starts_with(expected)) {
        return Err(corrupt("文件头不是 .bdict"));
    }
    Ok(())
}

fn utf16le(bytes: &[u8]) -> String {
    let units = bytes
        .chunks(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&units)
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    // 调用方已保证头部长度（HEADER_SIZE 覆盖 0x40/0x44/0x60）。
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    read_u32(bytes, offset) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(syllable: &str) -> (u8, u8) {
        let initial = INITIALS
            .iter()
            .enumerate()
            .filter(|(_, text)| !text.is_empty() && syllable.starts_with(*text))
            .max_by_key(|(_, text)| text.len())
            .map(|(index, _)| index)
            .expect("未知声母");
        let rest = syllable.strip_prefix(INITIALS[initial]).unwrap_or(syllable);
        let final_ = FINALS
            .iter()
            .position(|text| *text == rest)
            .expect("未知韵母");
        (initial as u8, final_ as u8)
    }

    fn write_entry(out: &mut Vec<u8>, syllables: &[&str], word: &str) {
        out.extend_from_slice(&(syllables.len() as i32).to_le_bytes());
        for syllable in syllables {
            let (initial, final_) = pair(syllable);
            out.push(initial);
            out.push(final_);
        }
        for unit in word.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
    }

    fn build_sample() -> Vec<u8> {
        let mut body = Vec::new();
        write_entry(&mut body, &["fu", "jian", "tu", "lou"], "福建土楼");
        // 混排词条：英文字母用 0xFF + ASCII
        body.extend_from_slice(&6i32.to_le_bytes());
        for (initial, final_) in [(1u8, 27u8), (19, 8)] {
            body.push(initial);
            body.push(final_);
        }
        for ch in "golf".chars() {
            body.push(INITIAL_ENGLISH);
            body.push(ch as u8);
        }
        for unit in "大众golf".encode_utf16() {
            body.extend_from_slice(&unit.to_le_bytes());
        }
        write_entry(&mut body, &["zhu"], "#");
        write_entry(&mut body, &["fu", "jian", "tu", "lou"], "福建土楼");
        write_entry(&mut body, &["hao"], "好");

        let mut out = vec![0u8; HEADER_SIZE];
        out[..8].copy_from_slice(b"biptbdsw");
        out[OFFSET_DATA_START..OFFSET_DATA_START + 4]
            .copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        out[OFFSET_DATA_LENGTH..OFFSET_DATA_LENGTH + 4]
            .copy_from_slice(&(body.len() as u32).to_le_bytes());
        out[OFFSET_DATA_END..OFFSET_DATA_END + 4]
            .copy_from_slice(&((HEADER_SIZE + body.len()) as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn parses_synthetic_bdict() {
        let parsed = BaiduBdict.parse(&build_sample()).expect("parse");
        assert_eq!(parsed.format_id, "baidu-bdict");
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].word, "福建土楼");
        assert_eq!(parsed.entries[0].code, "fu jian tu lou");
        assert_eq!(parsed.entries[1].word, "好");
        assert_eq!(parsed.entries[1].code, "hao");
        assert_eq!(parsed.skipped, 2);
        assert_eq!(parsed.duplicates, 1);
    }

    #[test]
    fn parses_real_qiche_bdict() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bdict/qiche.bdict"
        ))
        .expect("fixture");
        let parsed = BaiduBdict.parse(&bytes).expect("parse");
        assert_eq!(parsed.entries.len(), 4105);
        assert_eq!(parsed.skipped, 1);
        assert_eq!(parsed.duplicates, 0);
        assert_eq!(parsed.entries[0].word, "江铃轻卡");
        assert_eq!(parsed.entries[0].code, "jiang ling qing ka");
        let last = parsed.entries.last().unwrap();
        assert_eq!(last.word, "二轴二档轮");
        assert_eq!(last.code, "er zhou er dang lun");
        assert!(parsed.entries.iter().all(|entry| entry
            .code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b' ')));
        let long = parsed
            .entries
            .iter()
            .find(|entry| entry.word == "北京来宝汽车美容学校")
            .expect("long word");
        assert_eq!(long.code, "bei jing lai bao qi che mei rong xue xiao");
    }

    #[test]
    fn rejects_unknown_magic() {
        let mut bytes = build_sample();
        bytes[0] = b'x';
        assert!(BaiduBdict.parse(&bytes).is_err());
    }

    #[test]
    fn rejects_truncated_entries() {
        let bytes = build_sample();
        let cut = bytes.len() - 4;
        assert!(BaiduBdict.parse(&bytes[..cut]).is_err());
    }

    #[test]
    fn rejects_absurd_syllable_count() {
        let mut bytes = build_sample();
        bytes[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&100_000i32.to_le_bytes());
        assert!(BaiduBdict.parse(&bytes).is_err());
    }
}
