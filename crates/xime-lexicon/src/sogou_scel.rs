//! 搜狗细胞词库（`.scel`）解析。
//!
//! 二进制格式与 imewlconverter 的 `SougouScelImporter` 一致：
//! - `0x120`：Int32 词条分组数；
//! - `0x1540`：Int32 拼音表条数，随后每条为 (UInt16 索引, UInt16 字节长度, UTF-16LE 拼音)；
//! - 之后是词条分组：(UInt16 同音词数, UInt16 拼音索引字节数, 索引…, 词…)；
//! - 每条词后固定 12 字节扩展信息（2 + 4 + 6）。
//!
//! 该格式自带拼音，解析后直接得到 Rime 全拼码；没有可靠词频，统一低权重。

use crate::support::DEFAULT_WEIGHT;
use crate::{CodeSystem, Entry, Format, LexiconError, ParsedLexicon, corrupt};
use std::collections::HashMap;

pub(crate) struct SogouScel;

const OFFSET_DICT_LEN: usize = 0x120;
const OFFSET_PINYIN_TABLE: usize = 0x1540;
const MAX_GROUPS: i32 = 1_000_000;
const MAX_PINYIN_TABLE: i32 = 200_000;
const MAX_PINYIN_BYTES: usize = 1024;
const MAX_ENTRIES: usize = 500_000;
const WORD_EXTRA_BYTES: usize = 12;

impl Format for SogouScel {
    fn id(&self) -> &'static str {
        "sogou-scel"
    }

    fn display_name(&self) -> &'static str {
        "搜狗细胞词库"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".scel"]
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

/// 解析后的一行原始词条。
struct RawEntry {
    word: String,
    code: String,
}

struct Conversion {
    entries: Vec<Entry>,
    skipped: usize,
    duplicates: usize,
}

/// 原始分词 -> Rime 词条：去空、去重、过滤会破坏码表行的字符。
fn convert(raw: &[RawEntry]) -> Conversion {
    let mut seen = std::collections::HashSet::with_capacity(raw.len() * 2);
    let mut entries = Vec::with_capacity(raw.len());
    let mut skipped = 0;
    let mut duplicates = 0;

    for entry in raw {
        let code = entry.code.trim();
        let word = entry.word.trim();
        if word.is_empty() || code.is_empty() {
            skipped += 1;
            continue;
        }
        if word.starts_with('#') || entry.word.contains(['\t', '\n', '\r']) {
            skipped += 1;
            continue;
        }
        if !seen.insert((entry.word.clone(), code.to_string())) {
            duplicates += 1;
            continue;
        }
        entries.push(Entry {
            word: entry.word.clone(),
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
    if bytes.len() < OFFSET_PINYIN_TABLE + 4 {
        return Err(corrupt("文件太小，不是有效的 .scel"));
    }

    let mut reader = Reader::new(bytes);
    reader.seek(OFFSET_DICT_LEN)?;
    let group_count = reader.read_i32()?;
    reader.seek(OFFSET_PINYIN_TABLE)?;
    let pinyin_count = reader.read_i32()?;

    if !(0..=MAX_GROUPS).contains(&group_count) {
        return Err(corrupt(format!("词条数异常：{group_count}")));
    }
    if !(0..=MAX_PINYIN_TABLE).contains(&pinyin_count) {
        return Err(corrupt(format!("拼音表条数异常：{pinyin_count}")));
    }

    let mut table: HashMap<u16, String> = HashMap::with_capacity(pinyin_count as usize * 2);
    for _ in 0..pinyin_count {
        let index = reader.read_u16()?;
        let byte_len = reader.read_u16()? as usize;
        if !byte_len.is_multiple_of(2) || !reader.can_read(byte_len) {
            return Err(corrupt("拼音表损坏"));
        }
        table.insert(index, reader.read_utf16(byte_len)?);
    }

    let table_size = table.len();
    let mut entries = Vec::new();
    for _ in 0..group_count {
        let same_pinyin = reader.read_u16()? as usize;
        let pinyin_bytes = reader.read_u16()? as usize;
        if !pinyin_bytes.is_multiple_of(2)
            || pinyin_bytes > MAX_PINYIN_BYTES
            || !reader.can_read(pinyin_bytes)
        {
            return Err(corrupt("拼音索引损坏"));
        }

        let mut syllables = Vec::with_capacity(pinyin_bytes / 2);
        for _ in 0..pinyin_bytes / 2 {
            let key = reader.read_u16()?;
            let syllable = table
                .get(&key)
                .cloned()
                .unwrap_or_else(|| fallback_syllable(key, table_size));
            syllables.push(syllable);
        }
        let code = syllables
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");

        for _ in 0..same_pinyin {
            let word_bytes = reader.read_u16()? as usize;
            if !word_bytes.is_multiple_of(2) || !reader.can_read(word_bytes) {
                return Err(corrupt("词条长度损坏"));
            }
            let word = reader.read_utf16(word_bytes)?;
            if !reader.can_read(WORD_EXTRA_BYTES) {
                return Err(corrupt("词条扩展信息不完整"));
            }
            reader.skip(WORD_EXTRA_BYTES)?;
            if !word.is_empty() && !code.is_empty() {
                entries.push(RawEntry { word, code: code.clone() });
            }
            if entries.len() > MAX_ENTRIES {
                return Err(corrupt("词条数量超过上限"));
            }
        }
    }
    Ok(entries)
}

/// 与参考实现一致：索引超出拼音表时按字母表回退。
fn fallback_syllable(key: u16, table_size: usize) -> String {
    let code = i32::from(key) - table_size as i32 + i32::from(b'a');
    char::from_u32(code as u32)
        .filter(|c| c.is_ascii_lowercase())
        .map(|c| c.to_string())
        .unwrap_or_default()
}

/// 带边界检查的小端读取器。
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn seek(&mut self, offset: usize) -> Result<(), LexiconError> {
        if offset > self.data.len() {
            return Err(corrupt(format!("偏移越界：{offset}")));
        }
        self.pos = offset;
        Ok(())
    }

    fn can_read(&self, count: usize) -> bool {
        self.pos
            .checked_add(count)
            .is_some_and(|end| end <= self.data.len())
    }

    fn skip(&mut self, count: usize) -> Result<(), LexiconError> {
        if !self.can_read(count) {
            return Err(corrupt("数据提前结束"));
        }
        self.pos += count;
        Ok(())
    }

    fn read_u16(&mut self) -> Result<u16, LexiconError> {
        if !self.can_read(2) {
            return Err(corrupt("数据提前结束"));
        }
        let value = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(value)
    }

    fn read_i32(&mut self) -> Result<i32, LexiconError> {
        if !self.can_read(4) {
            return Err(corrupt("数据提前结束"));
        }
        let value = i32::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(value)
    }

    fn read_utf16(&mut self, byte_count: usize) -> Result<String, LexiconError> {
        if !self.can_read(byte_count) {
            return Err(corrupt("数据提前结束"));
        }
        let units = self.data[self.pos..self.pos + byte_count]
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        self.pos += byte_count;
        Ok(String::from_utf16_lossy(&units))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_scelexport() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/scel/errenzhuan.scel"
        ))
        .expect("fixture");
        let parsed = SogouScel.parse(&bytes).expect("parse");
        assert_eq!(parsed.format_id, "sogou-scel");
        assert_eq!(parsed.code_system, CodeSystem::Pinyin);
        assert_eq!(parsed.entries.len(), 2615);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.duplicates, 0);
        assert_eq!(parsed.entries[0].word, "矮子步");
        assert_eq!(parsed.entries[0].code, "ai zi bu");
        assert_eq!(parsed.entries.last().unwrap().word, "坐中央");
        assert_eq!(parsed.entries.last().unwrap().code, "zuo zhong yang");
        let long = parsed
            .entries
            .iter()
            .find(|entry| entry.word == "八个姑娘打擂台")
            .expect("long word");
        assert_eq!(long.code, "ba ge gu niang da lei tai");
    }

    #[test]
    fn rejects_truncated_file() {
        let error = SogouScel.parse(&[0u8; 16]).expect_err("too small");
        assert!(error.to_string().contains("太小"));
    }

    #[test]
    fn convert_skips_empty_and_dedupes() {
        let raw = vec![
            RawEntry { word: "好".into(), code: "hao".into() },
            RawEntry { word: "好".into(), code: "hao".into() },
            RawEntry { word: "".into(), code: "hao".into() },
            RawEntry { word: "#注释".into(), code: "zhu shi".into() },
            RawEntry { word: "行\t列".into(), code: "hang lie".into() },
        ];
        let converted = convert(&raw);
        assert_eq!(converted.entries.len(), 1);
        assert_eq!(converted.skipped, 3);
        assert_eq!(converted.duplicates, 1);
    }
}
