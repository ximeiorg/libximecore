//! 外部词库解析：把各家输入法的词库读成「词 + Rime 码 + 权重」。
//!
//! 本 crate 只做格式解析与归一化，不写文件、不碰 Rime 目录。
//! 「解析结果 → 派生 `.dict.yaml` → 挂到方案」在 `xime-config::imported_dict`。
//!
//! 支持的格式（对齐 Android 端 lexicon-core）：
//!
//! | id | 格式 | 扩展名 | 自带拼音 |
//! |----|------|--------|----------|
//! | `sogou-scel` | 搜狗细胞词库 | `.scel` | 是 |
//! | `baidu-bdict` | 百度分类词库 | `.bdict` | 是 |
//! | `libimebin` | Fcitx5/libime 二进制词库 | `.dict` | 是 |
//! | `sgpybin` | 搜狗拼音备份词库 | `.bin` | 是 |
//! | `libimetxt` | Fcitx5/libime 文本词库 | `.txt` | 是 |
//! | `sgpy` | 搜狗拼音文本词库 | `.txt` | 是 |
//! | `bdpy` | 百度拼音文本词库 | `.txt` | 是 |
//!
//! `.txt` 被三种文本格式共用，扩展名不足以区分，靠 [`detect`] 读内容判断。

mod baidu_bdict;
mod libime_bin;
mod sogou_bin;
mod sogou_scel;
mod support;
mod text;

use std::fmt;
use std::path::Path;

pub use support::MAX_WEIGHT;

/// 词库的编码体系；导入时据此选择可挂载的方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeSystem {
    /// 拼音（Rime 码为空格分音节的全拼，如 `ni hao`）。
    Pinyin,
    /// 五笔等其他字根编码。
    Wubi,
    /// 未知。
    Other,
}

impl CodeSystem {
    /// 展示名（设置界面用）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Pinyin => "拼音",
            Self::Wubi => "五笔",
            Self::Other => "其他",
        }
    }

    /// 是否需要 `script_translator`（拼音方案的翻译器）。
    pub fn needs_script_translator(self) -> bool {
        self == Self::Pinyin
    }
}

/// 一条词条：词 + 已整理成 Rime 可用的码 + 权重。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub word: String,
    pub code: String,
    pub weight: i32,
}

/// 一次解析的结果；导入管线只依赖这个模型，不认具体格式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLexicon {
    pub format_id: &'static str,
    pub format_name: &'static str,
    pub code_system: CodeSystem,
    pub entries: Vec<Entry>,
    pub skipped: usize,
    pub duplicates: usize,
}

/// 词库内容不符合该格式（或格式损坏）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexiconError {
    message: String,
}

impl LexiconError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for LexiconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LexiconError {}

pub(crate) fn corrupt(message: impl Into<String>) -> LexiconError {
    LexiconError::new(message)
}

/// 单文件大小上限（64 MiB）：再大的「词库」基本都是选错文件了。
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// 词库格式解析器。新增格式：实现本 trait 并加进 [`formats`]。
///
/// 约定：
/// - [`Format::parse`] 只做格式解析和归一化，不写文件、不部署；
/// - 输出的 [`Entry::code`] 必须已经是目标编码体系下的 Rime 码
///   （拼音用空格分音节，如 `ni hao`）；
/// - 空词/空码/重复项由解析器自行处理，并通过 [`ParsedLexicon::skipped`] /
///   [`ParsedLexicon::duplicates`] 报告。
pub trait Format: Send + Sync {
    /// 稳定标识，用于记录/日志。
    fn id(&self) -> &'static str;
    /// 用户可读的格式名，如「搜狗细胞词库」。
    fn display_name(&self) -> &'static str;
    /// 认领的扩展名（含点，小写比较）。
    fn extensions(&self) -> &'static [&'static str];
    /// 单次解析接受的字节数上限。
    fn max_bytes(&self) -> u64 {
        MAX_BYTES
    }
    /// 输出词条的编码体系。
    fn code_system(&self) -> CodeSystem {
        CodeSystem::Pinyin
    }
    /// 按文件名认领格式；默认只比扩展名。
    fn matches(&self, file_name: &str) -> bool {
        let lower = file_name.to_ascii_lowercase();
        self.extensions()
            .iter()
            .any(|ext| lower.ends_with(&ext.to_ascii_lowercase()))
    }
    /// 内容特征判断，用于多种格式共用同一扩展名（如 `.txt`）时区分。
    /// 扩展名已能唯一确定的格式保持默认 `true`。
    fn looks_like(&self, bytes: &[u8]) -> bool {
        let _ = bytes;
        true
    }
    fn parse(&self, bytes: &[u8]) -> Result<ParsedLexicon, LexiconError>;
}

static FORMATS: &[&dyn Format] = &[
    &sogou_scel::SogouScel,
    &baidu_bdict::BaiduBdict,
    &libime_bin::LibimeBin,
    &sogou_bin::SogouBin,
    &text::baidu_pinyin::BaiduPinyin,
    &text::libime::LibimeText,
    &text::sogou_pinyin::SogouPinyin,
];

/// 已注册的格式（顺序即检测优先级）。
pub fn formats() -> &'static [&'static dyn Format] {
    FORMATS
}

/// 按扩展名认领该文件的所有格式。
pub fn candidates(file_name: &str) -> Vec<&'static dyn Format> {
    FORMATS
        .iter()
        .copied()
        .filter(|format| format.matches(file_name))
        .collect()
}

/// 是否存在认领该扩展名的格式。
pub fn has_candidate(file_name: &str) -> bool {
    FORMATS.iter().any(|format| format.matches(file_name))
}

/// 扩展名能唯一确定格式时返回解析器；`.txt` 这类多格式共用的返回 `None`。
pub fn find_by_file_name(file_name: &str) -> Option<&'static dyn Format> {
    let mut matched = candidates(file_name).into_iter();
    match (matched.next(), matched.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// 按内容在候选格式里挑一个；都不匹配返回 `None`。
pub fn detect(file_name: &str, bytes: &[u8]) -> Option<&'static dyn Format> {
    candidates(file_name)
        .into_iter()
        .find(|format| format.looks_like(bytes))
}

/// 给「不支持这种格式」提示用。
pub fn supported_description() -> String {
    FORMATS
        .iter()
        .map(|format| format.display_name())
        .collect::<Vec<_>>()
        .join("、")
}

/// 解析一段内存里的词库内容。
///
/// 扩展名能唯一确定格式时直接用该格式，否则按内容探测；
/// 两者都失败时返回带支持列表的错误。
pub fn parse_bytes(file_name: &str, bytes: &[u8]) -> Result<ParsedLexicon, LexiconError> {
    let format = find_by_file_name(file_name).or_else(|| detect(file_name, bytes));
    let Some(format) = format else {
        if has_candidate(file_name) {
            return Err(LexiconError::new(format!(
                "文件内容与支持的任何格式都不匹配（支持：{}）",
                supported_description()
            )));
        }
        return Err(LexiconError::new(format!(
            "不支持这种词库格式（支持：{}）",
            supported_description()
        )));
    };
    if bytes.is_empty() {
        return Err(LexiconError::new("文件是空的"));
    }
    if bytes.len() as u64 > format.max_bytes() {
        return Err(LexiconError::new(format!(
            "文件太大（{} MiB），{} 上限 {} MiB",
            bytes.len() / (1024 * 1024),
            format.display_name(),
            format.max_bytes() / (1024 * 1024)
        )));
    }
    format.parse(bytes)
}

/// 解析磁盘上的词库文件。
pub fn parse_file(path: &Path) -> Result<ParsedLexicon, LexiconError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let meta = std::fs::metadata(path)
        .map_err(|e| LexiconError::new(format!("读不了文件 {}：{e}", path.display())))?;
    if !meta.is_file() {
        return Err(LexiconError::new(format!(
            "{} 不是文件",
            path.display()
        )));
    }
    if meta.len() > MAX_BYTES {
        return Err(LexiconError::new(format!(
            "文件太大（{} MiB）",
            meta.len() / (1024 * 1024)
        )));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| LexiconError::new(format!("读取 {} 失败：{e}", path.display())))?;
    parse_bytes(&file_name, &bytes)
}
