//! Fcitx5 / libime 二进制拼音词库（`.dict`）解析。
//!
//! 文件布局（libime `pinyin/pinyindictionary.cpp`）：
//! - `0x00`：Big-Endian Int32 magic `0x000fc613`；
//! - `0x04`：Big-Endian Int32 版本，1 = 未压缩，2 = zstd 压缩（可多帧）；
//! - `0x08`：`DATrie<float>` 序列化数据。
//!
//! DATrie 布局（libime `core/datrie.cpp`）：
//! `[be32 tailLen][be32 nodeCount][tail][nodeCount * (be32 base, be32 check)]
//!  [be32 bheadF/bheadC/bheadO][nodeCount * (u8 sibling, u8 child)][blockCount * 20B]`，
//! 其中 `blockCount = nodeCount / 256`。
//!
//! 键为「每音节 2 字节拼音编码 + `!` + UTF-8 汉字」，值为 float 词频。

use crate::support::{DEFAULT_WEIGHT, MAX_WEIGHT};
use crate::{CodeSystem, Entry, Format, LexiconError, ParsedLexicon, corrupt};
use std::io::Read;

pub(crate) struct LibimeBin;

const HEADER_BYTES: usize = 8;
const MAGIC: i32 = 0x000f_c613;
const VERSION_LEGACY: i32 = 1;
const VERSION_ZSTD: i32 = 2;
const MAX_DATRIE_BYTES: usize = 256 * 1024 * 1024;
const HANZI_SEPARATOR: u8 = b'!';

impl Format for LibimeBin {
    fn id(&self) -> &'static str {
        "libimebin"
    }

    fn display_name(&self) -> &'static str {
        "Fcitx5/libime 二进制词库"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".dict"]
    }

    fn max_bytes(&self) -> u64 {
        64 * 1024 * 1024
    }

    fn code_system(&self) -> CodeSystem {
        CodeSystem::Pinyin
    }

    /// 只认完整 8 字节头 + libime magic，避免别的 `.dict` 被误认为 libime 词库。
    fn looks_like(&self, bytes: &[u8]) -> bool {
        bytes.len() >= HEADER_BYTES && read_be_i32(bytes, 0) == MAGIC
    }

    fn parse(&self, bytes: &[u8]) -> Result<ParsedLexicon, LexiconError> {
        let raw = read_datrie(bytes)?;
        let trie = CedarFloatTrie::load(&raw)?;
        let converted = convert(trie.entries()?);
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

struct Conversion {
    entries: Vec<Entry>,
    skipped: usize,
    duplicates: usize,
}

/// DATrie 键值 -> Rime 词条。
///
/// 键必须是「偶数个拼音字节 + `!` + 非空汉字」；音节解不出、键结构不对的整条跳过。
fn convert(raw: Vec<(Vec<u8>, f32)>) -> Conversion {
    let mut seen = std::collections::HashSet::with_capacity(raw.len());
    let mut entries = Vec::with_capacity(raw.len());
    let mut skipped = 0;
    let mut duplicates = 0;

    for (key, value) in raw {
        let separator = key.iter().position(|byte| *byte == HANZI_SEPARATOR);
        let Some(separator) = separator else {
            skipped += 1;
            continue;
        };
        if separator == 0 || separator % 2 != 0 || separator == key.len() - 1 {
            skipped += 1;
            continue;
        }
        let word = String::from_utf8_lossy(&key[separator + 1..]).into_owned();
        let syllables = decode_full_pinyin(&key[..separator]);
        let Some(syllables) = syllables else {
            skipped += 1;
            continue;
        };
        let code = syllables.join(" ");
        if word.is_empty() || code.is_empty() {
            skipped += 1;
            continue;
        }
        if !seen.insert((word.clone(), code.clone())) {
            duplicates += 1;
            continue;
        }
        entries.push(Entry {
            word,
            code,
            weight: weight_of(value),
        });
    }

    Conversion {
        entries,
        skipped,
        duplicates,
    }
}

/// libime 存 float 词频；非正/非有限值视为没有词频，用默认权重。
fn weight_of(value: f32) -> i32 {
    if !value.is_finite() || value <= 1.0 {
        return DEFAULT_WEIGHT;
    }
    (value as i64).clamp(i64::from(DEFAULT_WEIGHT), i64::from(MAX_WEIGHT)) as i32
}

/// libime 拼音编码表（`libime/pinyin/pinyindata.cpp` 规范条目）的解码方向。
///
/// 每个音节存成 (声母, 韵母) 两个字符，取值从 `'A'` 起按 libime 枚举顺序编号。
const PINYIN_INITIALS: [&str; 24] = [
    "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "zh", "ch", "sh", "r", "z",
    "c", "s", "y", "w", "",
];

const PINYIN_FINALS: [&str; 62] = [
    "a", "ai", "an", "ang", "ao", "e", "ei", "en", "eng", "er", "o", "ong", "ou", "i", "ia", "ie",
    "iao", "iu", "ian", "in", "iang", "ing", "iong", "u", "ua", "uo", "uai", "ui", "uan", "un",
    "uang", "v", "ve", "ue", "ng", "", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l",
    "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
];

const INITIAL_BASE: u8 = b'A';
const FINAL_BASE: u8 = b'A';

/// 解出全部音节；任一音节无法识别时返回 `None`（整条词目应跳过）。
fn decode_full_pinyin(data: &[u8]) -> Option<Vec<String>> {
    if data.is_empty() || !data.len().is_multiple_of(2) {
        return None;
    }
    let mut syllables = Vec::with_capacity(data.len() / 2);
    for pair in data.chunks(2) {
        let initial = PINYIN_INITIALS
            .get(pair[0].wrapping_sub(INITIAL_BASE) as usize)
            .copied()?;
        let final_ = PINYIN_FINALS
            .get(pair[1].wrapping_sub(FINAL_BASE) as usize)
            .copied()?;
        let syllable = format!("{initial}{final_}");
        if syllable.is_empty() {
            return None;
        }
        syllables.push(syllable);
    }
    Some(syllables)
}

/// libime 二进制容器头读取与 zstd 解压。
fn read_datrie(bytes: &[u8]) -> Result<Vec<u8>, LexiconError> {
    if bytes.len() < HEADER_BYTES {
        return Err(corrupt("文件太小，不是有效的 libime 二进制词库"));
    }
    let magic = read_be_i32(bytes, 0);
    if magic != MAGIC {
        return Err(corrupt(format!(
            "不是 libime 二进制词库：magic=0x{magic:08x}"
        )));
    }
    match read_be_i32(bytes, 4) {
        VERSION_LEGACY => Ok(bytes[HEADER_BYTES..].to_vec()),
        VERSION_ZSTD => unzstd(&bytes[HEADER_BYTES..]),
        version => Err(corrupt(format!(
            "不支持的 libime 二进制版本：{}",
            version as u32
        ))),
    }
}

/// 解压 zstd 帧（可能多帧，尾部还可能跟着构建期的 block 链表）。
///
/// 尾部垃圾会让解码在读完所有帧后报错；此时只要已经拿到数据就继续——
/// 后续 DATrie 载入会按精确长度校验，被截断的数据不会蒙混过关。
fn unzstd(input: &[u8]) -> Result<Vec<u8>, LexiconError> {
    let decoder = zstd::stream::read::Decoder::new(input)
        .map_err(|e| corrupt(format!("zstd 解压失败：{e}")))?;
    let mut out = Vec::new();
    let mut limited = decoder.take(MAX_DATRIE_BYTES as u64 + 1);
    if let Err(error) = limited.read_to_end(&mut out) {
        if out.is_empty() {
            return Err(corrupt(format!("zstd 解压失败：{error}")));
        }
    }
    if out.len() > MAX_DATRIE_BYTES {
        return Err(corrupt(format!(
            "libime 词库解压后超过上限：{MAX_DATRIE_BYTES} 字节"
        )));
    }
    if out.is_empty() {
        return Err(corrupt("zstd 解压结果为空"));
    }
    Ok(out)
}

fn read_be_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

// ---- cedar 双数组 Trie（只读、只保留 Load / Enumerate 路径）----

const VALUE_SIZE: usize = 4;
const NO_VALUE: i32 = 0x7fc0_0001;
const NO_PATH: i32 = 0x7fc0_0002;
const BLOCK_SIZE: usize = 256;
const BLOCK_BYTES: usize = 20;
const MAX_NODES: usize = 8 * 1024 * 1024;
const MAX_TAIL_BYTES: usize = 64 * 1024 * 1024;
const MAX_KEY_BYTES: usize = 4096;

/// libime `DATrie<float>` 的只读移植。
///
/// 构建期的 block 链表与 bhead 只校验长度后跳过，遍历只用
/// `base`、`check`、`sibling`、`child` 与 `tail`。
struct CedarFloatTrie {
    tail: Vec<u8>,
    base: Vec<i32>,
    check: Vec<i32>,
    sibling: Vec<u8>,
    child: Vec<u8>,
}

#[derive(Default, Clone, Copy)]
struct Cursor {
    index: i32,
    offset: i32,
    length: i32,
}

impl CedarFloatTrie {
    /// 读取 DATrie 序列化数据；长度或节点数不符立即失败。
    fn load(data: &[u8]) -> Result<Self, LexiconError> {
        let mut reader = ByteReader::new(data);
        let tail_len = reader.read_i32_be()?;
        let node_count = reader.read_i32_be()?;

        if tail_len < VALUE_SIZE as i32 || tail_len as usize > MAX_TAIL_BYTES {
            return Err(corrupt(format!("DATrie tail 长度异常：{tail_len}")));
        }
        if node_count < BLOCK_SIZE as i32
            || !(node_count as usize).is_multiple_of(BLOCK_SIZE)
            || node_count as usize > MAX_NODES
        {
            return Err(corrupt(format!("DATrie 节点数异常：{node_count}")));
        }
        let node_count = node_count as usize;
        let tail_len = tail_len as usize;
        let block_count = node_count / BLOCK_SIZE;

        let expected = 8
            + tail_len
            + node_count * 8
            + 12
            + node_count * 2
            + block_count * BLOCK_BYTES;
        if expected != data.len() {
            return Err(corrupt(format!(
                "DATrie 长度不符：期望 {expected} 字节，实际 {} 字节",
                data.len()
            )));
        }

        let tail = reader.read_bytes(tail_len)?.to_vec();
        let mut base = Vec::with_capacity(node_count);
        let mut check = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            base.push(reader.read_i32_be()?);
            check.push(reader.read_i32_be()?);
        }
        reader.skip(12)?;
        // 序列化布局是 (sibling, child) 交错，不是两个独立数组。
        let mut sibling = Vec::with_capacity(node_count);
        let mut child = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            sibling.push(reader.read_bytes(1)?[0]);
            child.push(reader.read_bytes(1)?[0]);
        }
        reader.skip(block_count * BLOCK_BYTES)?;

        Ok(Self {
            tail,
            base,
            check,
            sibling,
            child,
        })
    }

    /// 按 cedar 的 DFS 顺序枚举全部键值（值为 float）。
    fn entries(&self) -> Result<Vec<(Vec<u8>, f32)>, LexiconError> {
        let mut cursor = Cursor::default();
        let max_steps = self.base.len() as i64 * 4 + 1024;
        let mut steps = 0i64;
        let mut out = Vec::new();

        let mut raw = self.begin(&mut cursor)?;
        while raw != NO_PATH {
            if raw != NO_VALUE {
                out.push((self.suffix(cursor)?, f32::from_bits(raw as u32)));
            }
            steps += 1;
            if steps > max_steps {
                return Err(corrupt("DATrie 遍历没有终止，数据可能已损坏"));
            }
            raw = self.next(&mut cursor)?;
        }
        Ok(out)
    }

    fn begin(&self, cursor: &mut Cursor) -> Result<i32, LexiconError> {
        let mut from = cursor.index;
        let mut node_base = if cursor.offset != 0 {
            -cursor.offset
        } else {
            self.base_at(from)?
        };
        if node_base >= 0 {
            let mut label = self.child_at(from)?;
            if from == 0 {
                label = self.sibling_at(node_base ^ label)?;
                if label == 0 {
                    return Ok(NO_PATH);
                }
            }
            while label != 0 && node_base >= 0 {
                from = node_base ^ label;
                node_base = self.base_at(from)?;
                label = self.child_at(from)?;
                cursor.length += 1;
                if cursor.length as usize > MAX_KEY_BYTES {
                    return Err(corrupt("DATrie 键长度异常"));
                }
            }
            if node_base >= 0 {
                cursor.index = from;
                return self.base_at(node_base ^ label);
            }
        }
        cursor.index = from;
        let tail_length = self.str_len(-node_base)?;
        cursor.offset = -node_base + tail_length;
        cursor.length += tail_length;
        self.load_tail_value(-node_base + tail_length + 1)
    }

    fn next(&self, cursor: &mut Cursor) -> Result<i32, LexiconError> {
        let mut label = 0;
        if cursor.offset != 0 {
            let offset = cursor.offset;
            cursor.offset = 0;
            cursor.length -= offset - (-self.base_at(cursor.index)?);
        } else {
            label = self.sibling_at(self.base_at(cursor.index)?)?;
        }
        while label == 0 && !(cursor.offset == 0 && cursor.index == 0) {
            label = self.sibling_at(cursor.index)?;
            cursor.index = self.check_at(cursor.index)?;
            cursor.length -= 1;
            if cursor.length < 0 {
                return Err(corrupt("DATrie 回溯越界，数据可能已损坏"));
            }
        }
        if label == 0 {
            return Ok(NO_PATH);
        }
        cursor.index = self.base_at(cursor.index)? ^ label;
        cursor.length += 1;
        self.begin(cursor)
    }

    fn suffix(&self, cursor: Cursor) -> Result<Vec<u8>, LexiconError> {
        let mut length = cursor.length;
        if length < 0 || length as usize > MAX_KEY_BYTES {
            return Err(corrupt(format!("DATrie 键长度异常：{length}")));
        }
        let mut key = vec![0u8; length as usize];
        let mut to = cursor.index;
        let offset = cursor.offset;
        if offset != 0 {
            let mut tail_length = self.str_len(-self.base_at(to)?)?;
            if length > tail_length {
                length -= tail_length;
            } else {
                tail_length = length;
                length = 0;
            }
            let start = offset - tail_length;
            if start < 0 || (start + tail_length) as usize > self.tail.len() {
                return Err(corrupt("DATrie tail 越界"));
            }
            key[length as usize..(length + tail_length) as usize]
                .copy_from_slice(&self.tail[start as usize..(start + tail_length) as usize]);
        }
        while length > 0 {
            length -= 1;
            let from = self.check_at(to)?;
            key[length as usize] = (self.base_at(from)? ^ to) as u8;
            to = from;
        }
        Ok(key)
    }

    fn str_len(&self, offset: i32) -> Result<i32, LexiconError> {
        if offset < 0 || offset as usize >= self.tail.len() {
            return Err(corrupt(format!("DATrie tail 偏移越界：{offset}")));
        }
        let mut length = 0usize;
        while (offset as usize) + length < self.tail.len() {
            if self.tail[offset as usize + length] == 0 {
                return Ok(length as i32);
            }
            length += 1;
        }
        Err(corrupt("DATrie tail 字符串未结束"))
    }

    fn load_tail_value(&self, offset: i32) -> Result<i32, LexiconError> {
        if offset < 0 || offset as usize + VALUE_SIZE > self.tail.len() {
            return Err(corrupt(format!("DATrie tail 词频越界：{offset}")));
        }
        let start = offset as usize;
        Ok(i32::from_le_bytes([
            self.tail[start],
            self.tail[start + 1],
            self.tail[start + 2],
            self.tail[start + 3],
        ]))
    }

    fn base_at(&self, node: i32) -> Result<i32, LexiconError> {
        if node < 0 || node as usize >= self.base.len() {
            return Err(corrupt(format!("DATrie 节点越界：{node}")));
        }
        Ok(self.base[node as usize])
    }

    fn check_at(&self, node: i32) -> Result<i32, LexiconError> {
        if node < 0 || node as usize >= self.check.len() {
            return Err(corrupt(format!("DATrie 节点越界：{node}")));
        }
        Ok(self.check[node as usize])
    }

    fn sibling_at(&self, node: i32) -> Result<i32, LexiconError> {
        if node < 0 || node as usize >= self.sibling.len() {
            return Err(corrupt(format!("DATrie 节点越界：{node}")));
        }
        Ok(i32::from(self.sibling[node as usize]))
    }

    fn child_at(&self, node: i32) -> Result<i32, LexiconError> {
        if node < 0 || node as usize >= self.child.len() {
            return Err(corrupt(format!("DATrie 节点越界：{node}")));
        }
        Ok(i32::from(self.child[node as usize]))
    }
}

/// 大端字节读取器（DATrie 序列化数据是大端）。
struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_i32_be(&mut self) -> Result<i32, LexiconError> {
        if self.data.len() - self.pos < 4 {
            return Err(corrupt("DATrie 数据提前结束"));
        }
        let value = read_be_i32(self.data, self.pos);
        self.pos += 4;
        Ok(value)
    }

    fn read_bytes(&mut self, count: usize) -> Result<&'a [u8], LexiconError> {
        if count > self.data.len().saturating_sub(self.pos) {
            return Err(corrupt("DATrie 数据提前结束"));
        }
        let slice = &self.data[self.pos..self.pos + count];
        self.pos += count;
        Ok(slice)
    }

    fn skip(&mut self, count: usize) -> Result<(), LexiconError> {
        self.read_bytes(count).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/libimebin/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture")
    }

    #[test]
    fn parses_real_libime_dict() {
        let parsed = LibimeBin.parse(&fixture("web-slang.dict")).expect("parse");
        assert_eq!(parsed.format_id, "libimebin");
        assert_eq!(parsed.entries.len(), 423);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.duplicates, 0);
        assert_eq!(parsed.entries[0].word, "芭比Q了");
        assert_eq!(parsed.entries[0].code, "ba bi q le");
        assert_eq!(parsed.entries[1].word, "百合");
        assert_eq!(parsed.entries[1].code, "bai he");
        assert_eq!(parsed.entries[3].word, "板斧");
        assert_eq!(parsed.entries.last().unwrap().word, "XX控");
        assert_eq!(parsed.entries.last().unwrap().code, "x x kong");
        assert!(parsed
            .entries
            .iter()
            .all(|entry| entry.word.chars().count() == entry.code.split(' ').count()));
    }

    #[test]
    fn parses_downloaded_libime_dict() {
        let parsed = LibimeBin.parse(&fixture("web-slang-downloaded.dict")).expect("parse");
        assert_eq!(parsed.entries.len(), 351);
        assert_eq!(parsed.entries[0].word, "八千党");
        assert_eq!(parsed.entries[0].code, "ba qian dang");
        assert_eq!(parsed.entries.last().unwrap().word, "二共");
        assert_eq!(parsed.entries.last().unwrap().code, "er gong");
    }

    #[test]
    fn decompresses_whole_payload() {
        let raw = read_datrie(&fixture("web-slang.dict")).expect("datrie");
        assert_eq!(raw.len(), 17756);
    }

    #[test]
    fn only_recognizes_libime_magic() {
        let sample = fixture("web-slang.dict");
        assert!(LibimeBin.looks_like(&sample));
        assert!(!LibimeBin.looks_like(&[0u8; 8]));
        assert!(!LibimeBin.looks_like(&sample[..4]));
        let mut wrong = sample.clone();
        wrong[3] ^= 0x01;
        assert!(!LibimeBin.looks_like(&wrong));
        assert!(LibimeBin.parse(&wrong).is_err());
    }

    #[test]
    fn rejects_truncated_file() {
        let sample = fixture("web-slang.dict");
        for size in [8usize, 64, sample.len() / 2] {
            assert!(LibimeBin.parse(&sample[..size]).is_err(), "size={size}");
        }
    }

    #[test]
    fn parses_legacy_uncompressed_datrie() {
        let raw = read_datrie(&fixture("web-slang.dict")).expect("datrie");
        let mut legacy = vec![0x00, 0x0f, 0xc6, 0x13, 0x00, 0x00, 0x00, 0x01];
        legacy.extend_from_slice(&raw);
        let parsed = LibimeBin.parse(&legacy).expect("parse");
        assert_eq!(parsed.entries.len(), 423);
    }

    #[test]
    fn skips_invalid_keys_and_dedupes() {
        let valid = "AA!好".as_bytes().to_vec(); // 65,65 -> b + a
        let raw = vec![
            (valid.clone(), 3.0f32),
            (valid.clone(), 3.0),
            (vec![0, 0, b'!'], 0.0),
            (vec![65, 65], 0.0),
            ("AAA!好".as_bytes().to_vec(), 0.0),
            ("AA!".as_bytes().to_vec(), 0.0),
        ];
        let converted = convert(raw);
        assert_eq!(converted.entries.len(), 1);
        assert_eq!(converted.entries[0].word, "好");
        assert_eq!(converted.entries[0].code, "ba");
        assert_eq!(converted.entries[0].weight, 3);
        assert_eq!(converted.skipped, 4);
        assert_eq!(converted.duplicates, 1);
    }

    #[test]
    fn caps_weight_and_falls_back_to_default() {
        let key = "AA!好".as_bytes().to_vec();
        let converted = convert(vec![
            (key.clone(), f32::NAN),
            (key.clone(), 0.5),
            (key.clone(), 9.9e9),
        ]);
        // 三份同词条同码，后两条算重复，只剩第一条
        assert_eq!(converted.duplicates, 2);
        assert_eq!(converted.entries[0].weight, DEFAULT_WEIGHT);

        let converted = convert(vec![(key, 9.9e9)]);
        assert_eq!(converted.entries[0].weight, MAX_WEIGHT);
    }
}
