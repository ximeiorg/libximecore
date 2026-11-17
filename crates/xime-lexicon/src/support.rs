//! 文本词库共用的词条转换与内容探测。

use crate::Entry;
use encoding_rs::Encoding;

/// 权重上限：超过这个值对候选排序已无区别，只会撑大码表。
pub const MAX_WEIGHT: i32 = 1_000_000;

/// 没有词频时的默认权重（与方案自带词比不出高低，统一压低）。
pub const DEFAULT_WEIGHT: i32 = 1;

/// 内容探测只读文件开头这么多字节。
const SNIFF_BYTES: usize = 64 * 1024;

/// 解析后的一行原始词条；`weight` 为 0 表示没有词频。
pub(crate) type Row = (String, String, i32);

/// 去重后的词条与统计。
pub(crate) struct Built {
    pub entries: Vec<Entry>,
    pub skipped: usize,
    pub duplicates: usize,
}

/// 词频（浮点，libime 文本词库）归一化。
pub(crate) fn weight_of_f64(rank: f64) -> i32 {
    if rank.is_finite() && rank > f64::from(DEFAULT_WEIGHT) {
        (rank as i64).min(i64::from(MAX_WEIGHT)) as i32
    } else {
        DEFAULT_WEIGHT
    }
}

/// 词频（整数，百度文本词库 / 搜狗备份）归一化。
pub(crate) fn weight_of_i64(rank: i64) -> i32 {
    if rank > i64::from(DEFAULT_WEIGHT) {
        rank.min(i64::from(MAX_WEIGHT)) as i32
    } else {
        DEFAULT_WEIGHT
    }
}

/// 词面/编码归一化并去重；空词、空码、控制字符、注释行按跳过处理。
pub(crate) fn build(rows: &[Row]) -> Built {
    let mut seen = std::collections::HashSet::with_capacity(rows.len() * 2);
    let mut entries = Vec::with_capacity(rows.len());
    let mut skipped = 0;
    let mut duplicates = 0;

    for (word, code, weight) in rows {
        let word = word.trim();
        let code = code.trim();
        if word.is_empty() || code.is_empty() {
            skipped += 1;
            continue;
        }
        if word.starts_with('#') || word.contains(['\t', '\n', '\r']) {
            skipped += 1;
            continue;
        }
        // 词 + 码 的组合唯一：同一词不同码是合法多音词条。
        if !seen.insert((word.to_string(), code.to_string())) {
            duplicates += 1;
            continue;
        }
        entries.push(Entry {
            word: word.to_string(),
            code: code.to_string(),
            weight: if *weight > 0 {
                (*weight).min(MAX_WEIGHT)
            } else {
                DEFAULT_WEIGHT
            },
        });
    }

    Built {
        entries,
        skipped,
        duplicates,
    }
}

/// 严格解码；非法字节返回 `None`（用于区分 UTF-8 / GBK / UTF-16LE 内容）。
///
/// 探测切片可能截断多字节字符，从尾部回退几个字节再试。
pub(crate) fn decode_strict(bytes: &[u8], encoding: &'static Encoding, limit: usize) -> Option<String> {
    let head = &bytes[..bytes.len().min(limit)];
    for cut in 0..=3.min(head.len()) {
        let slice = &head[..head.len() - cut];
        if let Some(text) = encoding.decode_without_bom_handling_and_without_replacement(slice) {
            let mut text = text.into_owned();
            if let Some(stripped) = text.strip_prefix('\u{feff}') {
                text = stripped.to_string();
            }
            return Some(text);
        }
    }
    None
}

/// 整文件严格解码（探测用 [`SNIFF_BYTES`] 限长）。
pub(crate) fn decode_head(bytes: &[u8], encoding: &'static Encoding) -> Option<String> {
    decode_strict(bytes, encoding, SNIFF_BYTES)
}

/// 拼音编码：音节用空格分隔、小写；不像拼音时返回空串。
pub(crate) fn pinyin_code<'a>(syllables: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for syllable in syllables {
        let syllable = syllable.trim().to_ascii_lowercase();
        if syllable.is_empty()
            || syllable.len() > 6
            || !syllable.bytes().all(|b| b.is_ascii_lowercase())
        {
            return String::new();
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&syllable);
    }
    out
}

/// 汉字（含扩展区/兼容区），用于把词库文本和普通英文文本区分开。
pub(crate) fn contains_cjk(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
    })
}

/// 判断文本是否整体像「2~3 列、第二列是拼音」的词库。
pub(crate) fn looks_like_pinyin_text(text: &str, tokenize: &dyn Fn(&str) -> Vec<String>) -> bool {
    let mut lines = 0usize;
    let mut matched = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        lines += 1;
        let tokens = tokenize(line);
        if (2..=3).contains(&tokens.len())
            && !tokens[0].trim().is_empty()
            && contains_cjk(&tokens[0])
            && !pinyin_code(
                tokens[1]
                    .split('\'')
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>(),
            )
            .is_empty()
        {
            matched += 1;
        }
    }
    lines > 0 && matched * 10 >= lines * 6
}
