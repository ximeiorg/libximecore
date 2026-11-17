//! 搜狗拼音备份词库（`.bin`）解析，对应 imewlconverter 的 `sgpybin`。
//!
//! 实际存在三种二进制形态：
//! - `SGPU`（0x55504753）：新版「导出词库」备份，索引区 + 词条区；
//! - 首 4 字节 0x554401D3：7.2 版以前的明文备份，`0x18` 处为词条数，`0x30` 起为分组数据；
//! - 旧版加密备份（usrDictV3）：没有魔数，按文件头的键/属性/数据偏移表 + 词条解密。
//!
//! 前两种在 [`Format::looks_like`] 里用魔数认领；usrDictV3 只在直接解析时兜底尝试。

use crate::support::weight_of_i64;
use crate::{CodeSystem, Entry, Format, LexiconError, ParsedLexicon, corrupt};

pub(crate) struct SogouBin;

const MAGIC_SGPU: u32 = 0x5550_4753;
const MAGIC_PLAIN_OLD: u32 = 0x5544_01D3;

impl Format for SogouBin {
    fn id(&self) -> &'static str {
        "sgpybin"
    }

    fn display_name(&self) -> &'static str {
        "搜狗拼音备份词库"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[".bin"]
    }

    fn max_bytes(&self) -> u64 {
        64 * 1024 * 1024
    }

    fn code_system(&self) -> CodeSystem {
        CodeSystem::Pinyin
    }

    fn looks_like(&self, bytes: &[u8]) -> bool {
        if bytes.len() < 4 {
            return false;
        }
        let magic = magic_of(bytes);
        magic == MAGIC_SGPU || magic == MAGIC_PLAIN_OLD
    }

    fn parse(&self, bytes: &[u8]) -> Result<ParsedLexicon, LexiconError> {
        let raw = parse_raw(bytes)?;
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

/// 解析后的一条原始词条；`frequency` 是备份里记录的词频，可能为 0。
struct RawEntry {
    word: String,
    code: String,
    frequency: i32,
}

struct Conversion {
    entries: Vec<Entry>,
    skipped: usize,
    duplicates: usize,
}

/// 原始词条 -> Rime 词条：去空、去重、过滤会破坏码表行的字符。
fn convert(raw: &[RawEntry]) -> Conversion {
    let mut seen = std::collections::HashSet::with_capacity(raw.len() * 2);
    let mut entries = Vec::with_capacity(raw.len());
    let mut skipped = 0;
    let mut duplicates = 0;

    for entry in raw {
        let code = entry.code.trim();
        if entry.word.trim().is_empty()
            || code.is_empty()
            || entry.word.contains(['\t', '\n', '\r'])
        {
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
            weight: weight_of_i64(i64::from(entry.frequency)),
        });
    }

    Conversion {
        entries,
        skipped,
        duplicates,
    }
}

fn parse_raw(bytes: &[u8]) -> Result<Vec<RawEntry>, LexiconError> {
    if bytes.len() < 4 {
        return Err(corrupt("文件太小，不是有效的搜狗拼音备份词库"));
    }
    match magic_of(bytes) {
        MAGIC_SGPU => sgpu::parse(bytes),
        MAGIC_PLAIN_OLD => plain_old::parse(bytes),
        _ => encrypted::parse(bytes),
    }
}

fn magic_of(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// 搜狗拼音音节表；前 413 项是正常音节，之后是单字母、数字和 `#`（新格式用）。
const PINYIN_VALUES: &[&str] = &[
    "a", "ai", "an", "ang", "ao", "ba", "bai", "ban", "bang", "bao", "bei", "ben", "beng", "bi",
    "bian", "biao", "bie", "bin", "bing", "bo", "bu", "ca", "cai", "can", "cang", "cao", "ce",
    "cen", "ceng", "cha", "chai", "chan", "chang", "chao", "che", "chen", "cheng", "chi", "chong",
    "chou", "chu", "chua", "chuai", "chuan", "chuang", "chui", "chun", "chuo", "ci", "cong", "cou",
    "cu", "cuan", "cui", "cun", "cuo", "da", "dai", "dan", "dang", "dao", "de", "dei", "den",
    "deng", "di", "dia", "dian", "diao", "die", "ding", "diu", "dong", "dou", "du", "duan", "dui",
    "dun", "duo", "e", "ei", "en", "eng", "er", "fa", "fan", "fang", "fei", "fen", "feng", "fiao",
    "fo", "fou", "fu", "ga", "gai", "gan", "gang", "gao", "ge", "gei", "gen", "geng", "gong",
    "gou", "gu", "gua", "guai", "guan", "guang", "gui", "gun", "guo", "ha", "hai", "han", "hang",
    "hao", "he", "hei", "hen", "heng", "hong", "hou", "hu", "hua", "huai", "huan", "huang", "hui",
    "hun", "huo", "ji", "jia", "jian", "jiang", "jiao", "jie", "jin", "jing", "jiong", "jiu", "ju",
    "juan", "jue", "jun", "ka", "kai", "kan", "kang", "kao", "ke", "kei", "ken", "keng", "kong",
    "kou", "ku", "kua", "kuai", "kuan", "kuang", "kui", "kun", "kuo", "la", "lai", "lan", "lang",
    "lao", "le", "lei", "leng", "li", "lia", "lian", "liang", "liao", "lie", "lin", "ling", "liu",
    "lo", "long", "lou", "lu", "luan", "lve", "lun", "luo", "lv", "ma", "mai", "man", "mang",
    "mao", "me", "mei", "men", "meng", "mi", "mian", "miao", "mie", "min", "ming", "miu", "mo",
    "mou", "mu", "na", "nai", "nan", "nang", "nao", "ne", "nei", "nen", "neng", "ni", "nian",
    "niang", "niao", "nie", "nin", "ning", "niu", "nong", "nou", "nu", "nuan", "nve", "nun", "nuo",
    "nv", "o", "ou", "pa", "pai", "pan", "pang", "pao", "pei", "pen", "peng", "pi", "pian", "piao",
    "pie", "pin", "ping", "po", "pou", "pu", "qi", "qia", "qian", "qiang", "qiao", "qie", "qin",
    "qing", "qiong", "qiu", "qu", "quan", "que", "qun", "ran", "rang", "rao", "re", "ren", "reng",
    "ri", "rong", "rou", "ru", "rua", "ruan", "rui", "run", "ruo", "sa", "sai", "san", "sang",
    "sao", "se", "sen", "seng", "sha", "shai", "shan", "shang", "shao", "she", "shei", "shen",
    "sheng", "shi", "shou", "shu", "shua", "shuai", "shuan", "shuang", "shui", "shun", "shuo",
    "si", "song", "sou", "su", "suan", "sui", "sun", "suo", "ta", "tai", "tan", "tang", "tao",
    "te", "tei", "teng", "ti", "tian", "tiao", "tie", "ting", "tong", "tou", "tu", "tuan", "tui",
    "tun", "tuo", "wa", "wai", "wan", "wang", "wei", "wen", "weng", "wo", "wu", "xi", "xia",
    "xian", "xiang", "xiao", "xie", "xin", "xing", "xiong", "xiu", "xu", "xuan", "xue", "xun",
    "ya", "yan", "yang", "yao", "ye", "yi", "yin", "ying", "yo", "yong", "you", "yu", "yuan",
    "yue", "yun", "za", "zai", "zan", "zang", "zao", "ze", "zei", "zen", "zeng", "zha", "zhai",
    "zhan", "zhang", "zhao", "zhe", "zhei", "zhen", "zheng", "zhi", "zhong", "zhou", "zhu",
    "zhua", "zhuai", "zhuan", "zhuang", "zhui", "zhun", "zhuo", "zi", "zong", "zou", "zu", "zuan",
    "zui", "zun", "zuo", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o",
    "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", "0", "1", "2", "3", "4", "5", "6", "7",
    "8", "9", "#",
];

/// 把索引序列转成空格分隔的音节；越界索引直接忽略。
fn syllables_of(indexes: &[u16]) -> String {
    let mut out = String::new();
    for index in indexes {
        if let Some(value) = PINYIN_VALUES.get(*index as usize) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(value);
        }
    }
    out
}

/// 带边界检查的小端读取器；所有越界都转成 [`LexiconError`]。
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn size(&self) -> i64 {
        self.data.len() as i64
    }

    fn position(&self) -> i64 {
        self.pos as i64
    }

    /// 偏移可以是任何整数：负数/越界都按格式损坏处理。
    fn seek(&mut self, offset: i64) -> Result<(), LexiconError> {
        if offset < 0 || offset > self.size() {
            return Err(corrupt(format!("偏移越界：{offset}")));
        }
        self.pos = offset as usize;
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
        Ok(self.read_u32()? as i32)
    }

    fn read_u32(&mut self) -> Result<u32, LexiconError> {
        if !self.can_read(4) {
            return Err(corrupt("数据提前结束"));
        }
        let value = u32::from_le_bytes([
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

/// 新版 SGPU 备份：`0x38` 处是索引/词条区的位置与数量。
mod sgpu {
    use super::*;

    const HEADER_OFFSET: i64 = 0x38;
    const MAX_WORD_COUNT: u32 = 500_000;
    const MAX_PINYIN_BYTES: u16 = 8_192;

    pub(super) fn parse(bytes: &[u8]) -> Result<Vec<RawEntry>, LexiconError> {
        let mut reader = Reader::new(bytes);
        reader.seek(HEADER_OFFSET)?;

        let idx_begin = reader.read_u32()? as i64;
        let idx_size = reader.read_u32()? as i64;
        let word_count = reader.read_u32()?;
        let dict_begin = reader.read_u32()? as i64;
        reader.read_u32()?; // 词条区预分配容量，未使用
        let dict_size = reader.read_u32()? as i64;

        let file_size = reader.size();
        if idx_begin > file_size || idx_size > file_size - idx_begin {
            return Err(corrupt("索引区越界"));
        }
        if word_count > MAX_WORD_COUNT || i64::from(word_count) > idx_size / 4 {
            return Err(corrupt(format!("词条数异常：{word_count}")));
        }
        if dict_begin > file_size || dict_size > file_size - dict_begin {
            return Err(corrupt("词条区越界"));
        }

        let mut entries = Vec::with_capacity(word_count as usize);
        for index in 0..i64::from(word_count) {
            reader.seek(idx_begin + 4 * index)?;
            let record_offset = reader.read_u32()? as i64;
            if record_offset >= dict_size {
                return Err(corrupt(format!("词条偏移越界：{record_offset}")));
            }

            reader.seek(dict_begin + record_offset)?;
            let frequency = i32::from(reader.read_u16()?);
            reader.skip(2)?;
            reader.skip(5)?;

            let pinyin_bytes = reader.read_u16()?;
            if !pinyin_bytes.is_multiple_of(2) || pinyin_bytes > MAX_PINYIN_BYTES {
                return Err(corrupt(format!("拼音长度异常：{pinyin_bytes}")));
            }
            let mut indexes = Vec::with_capacity(pinyin_bytes as usize / 2);
            for _ in 0..pinyin_bytes / 2 {
                indexes.push(reader.read_u16()?);
            }
            reader.skip(2)?;

            let word_bytes = reader.read_u16()? as usize;
            if !word_bytes.is_multiple_of(2) {
                return Err(corrupt(format!("词条长度异常：{word_bytes}")));
            }
            let word = reader.read_utf16(word_bytes)?;
            entries.push(RawEntry {
                word,
                code: syllables_of(&indexes),
                frequency,
            });
        }
        Ok(entries)
    }
}

/// 7.2 版以前的明文备份（首 4 字节 0x554401D3）。
///
/// `0x18`：Int32 词条总数；`0x30` 起按分组：
/// (UInt16 同音词数, UInt16 未知, UInt16 拼音字节数, UInt16 音节索引…)，
/// 每个词条是 (UInt16 词字节数, UTF-16LE 词, UInt16 词频, UInt16 未知, Int32 未知)。
mod plain_old {
    use super::*;

    const WORD_COUNT_OFFSET: i64 = 0x18;
    const DATA_OFFSET: i64 = 0x30;
    const MAX_WORD_COUNT: i32 = 500_000;
    const MAX_PINYIN_BYTES: u16 = 8_192;

    pub(super) fn parse(bytes: &[u8]) -> Result<Vec<RawEntry>, LexiconError> {
        let mut reader = Reader::new(bytes);
        reader.seek(WORD_COUNT_OFFSET)?;
        let word_count = reader.read_i32()?;
        if !(0..=MAX_WORD_COUNT).contains(&word_count) {
            return Err(corrupt(format!("词条数异常：{word_count}")));
        }

        reader.seek(DATA_OFFSET)?;
        let mut entries: Vec<RawEntry> = Vec::with_capacity(word_count as usize);
        while (entries.len() as i32) < word_count {
            let same_pinyin = reader.read_u16()?;
            reader.skip(2)?;
            let pinyin_bytes = reader.read_u16()?;
            if same_pinyin == 0 || pinyin_bytes % 2 != 0 || pinyin_bytes > MAX_PINYIN_BYTES {
                return Err(corrupt("分组数据损坏"));
            }

            let mut indexes = Vec::with_capacity(pinyin_bytes as usize / 2);
            for _ in 0..pinyin_bytes / 2 {
                indexes.push(reader.read_u16()?);
            }
            let code = syllables_of(&indexes);
            let code_syllables = if code.is_empty() {
                0
            } else {
                code.matches(' ').count() + 1
            };

            for _ in 0..same_pinyin {
                let word_bytes = reader.read_u16()? as usize;
                if !word_bytes.is_multiple_of(2) {
                    return Err(corrupt(format!("词条长度异常：{word_bytes}")));
                }
                let word = reader.read_utf16(word_bytes)?;
                let frequency = i32::from(reader.read_u16()?);
                reader.skip(6)?;

                // 旧实现要求音节数等于字的个数，否则视为无效数据。
                let valid = code_syllables > 0 && code_syllables == word.chars().count();
                entries.push(RawEntry {
                    word,
                    code: if valid { code.clone() } else { String::new() },
                    frequency,
                });
            }
        }
        Ok(entries)
    }
}

/// 旧版加密备份（usrDictV3），逐段对应 imewlconverter 的 `SogouBinParser` 旧格式分支。
///
/// 文件头没有魔数，只能先校验偏移表再读取；任何越界都按格式损坏处理。
mod encrypted {
    use super::*;

    const USER_HEADER_SIZE: i64 = 0x4c;
    const MAX_KEYS: u32 = 64;
    const MAX_ATTRS: u32 = 4_096;
    const MAX_AINTS: u32 = 1_000_000;
    const MAX_HEADER_ITEMS: u32 = 65_536;
    const MAX_DATA_ITEMS: u32 = 4_096;
    const MAX_HASH_STORES: i32 = 100_000;
    const MAX_ATTR_COUNT: i32 = 500_000;
    const MAX_ENTRIES: usize = 500_000;
    const MAX_VISITS: i64 = 2_000_000;
    const MAX_CODE_UNITS: u16 = 4_096;

    const DATATYPE_HASH_SIZE: [i32; 8] = [0, 27, 414, 512, -1, -1, 512, 0];
    const KEY_ITEM_DATA_TYPE_SIZE: [i32; 16] = [4, 1, 1, 2, 1, 2, 2, 4, 4, 8, 4, 4, 4, 0, 0, 0];

    /// `0xFFFFFF8F` 作为 i32 的补码表示。
    const TYPE_DEF_MASK: i32 = -0x71;

    struct KeyItem {
        dict_type_def: u16,
        data_types: Vec<u16>,
        attr_idx: i32,
        key_data_idx: i32,
    }

    struct AttributeItem {
        count: i32,
        data_id: i32,
    }

    struct HeaderItem {
        offset: i64,
        data_size: i32,
        used_data_size: i32,
    }

    struct DictData {
        keys: Vec<KeyItem>,
        attributes: Vec<AttributeItem>,
        header_items_idx: Vec<HeaderItem>,
        header_items_attr: Vec<HeaderItem>,
        data_store: Vec<HeaderItem>,
        ds_base: i64,
        data_type_size: Vec<i32>,
        attr_size: Vec<i32>,
        base_hash_size: Vec<i32>,
    }

    pub(super) fn parse(bytes: &[u8]) -> Result<Vec<RawEntry>, LexiconError> {
        if (bytes.len() as i64) < USER_HEADER_SIZE {
            return Err(corrupt("文件太小，不是有效的搜狗旧版备份"));
        }
        let mut reader = Reader::new(bytes);
        let size = reader.size();

        reader.seek(4)?;
        let config_size = i64::from(reader.read_u32()?);
        let key_count = reader.read_u32()?;
        let attr_count = reader.read_u32()?;
        let aint_count = reader.read_u32()?;

        if config_size > size || config_size < 12 {
            return Err(corrupt(format!("配置区长度异常：{config_size}")));
        }
        if !(1..=MAX_KEYS).contains(&key_count)
            || !(1..=MAX_ATTRS).contains(&attr_count)
            || aint_count > MAX_AINTS
        {
            return Err(corrupt("词库表数量异常"));
        }

        reader.seek(20)?;
        let mut keys = Vec::with_capacity(key_count as usize);
        for _ in 0..key_count {
            keys.push(read_key(&mut reader, attr_count)?);
        }
        let mut attributes = Vec::with_capacity(attr_count as usize);
        for _ in 0..attr_count {
            attributes.push(read_attribute(&mut reader)?);
        }
        for _ in 0..aint_count {
            reader.read_u32()?;
        }

        let config_end = config_size + 8;
        if reader.position() != config_end {
            return Err(corrupt("配置区长度不一致"));
        }

        reader.seek(config_end + 8)?;
        reader.read_u32()?; // 总大小，只作校验用途，读取后忽略
        let idx_header_count = reader.read_u32()?;
        let attr_header_count = reader.read_u32()?;
        let data_header_count = reader.read_u32()?;
        if idx_header_count > MAX_HEADER_ITEMS
            || attr_header_count > MAX_HEADER_ITEMS
            || data_header_count > MAX_DATA_ITEMS
        {
            return Err(corrupt("数据表数量异常"));
        }

        let mut header_items_idx = Vec::with_capacity(idx_header_count as usize);
        for _ in 0..idx_header_count {
            header_items_idx.push(read_header_item(&mut reader)?);
        }
        let mut header_items_attr = Vec::with_capacity(attr_header_count as usize);
        for _ in 0..attr_header_count {
            header_items_attr.push(read_header_item(&mut reader)?);
        }
        let mut data_store = Vec::with_capacity(data_header_count as usize);
        for _ in 0..data_header_count {
            data_store.push(read_header_item(&mut reader)?);
        }
        let ds_base = reader.position();

        let header_size =
            12 * (i64::from(key_count) + i64::from(attr_count) + i64::from(aint_count)) + 24;
        if ds_base != config_end + header_size {
            return Err(corrupt("数据区偏移不一致"));
        }

        reader.seek(size - USER_HEADER_SIZE)?;
        reader.skip(56)?;
        let p2 = i64::from(reader.read_u32()?);
        let p3 = i64::from(reader.read_u32()?);

        let mut dict = DictData {
            keys,
            attributes,
            header_items_idx,
            header_items_attr,
            data_store,
            ds_base,
            data_type_size: Vec::new(),
            attr_size: Vec::new(),
            base_hash_size: Vec::new(),
        };
        dict.initialize()?;
        read_all_words(&mut reader, &dict, p2, p3, size)
    }

    fn read_key(reader: &mut Reader<'_>, attr_count: u32) -> Result<KeyItem, LexiconError> {
        let dict_type_def = reader.read_u16()?;
        let data_type_count = reader.read_u16()?;
        if data_type_count > 64 {
            return Err(corrupt("数据类型数量异常"));
        }
        let mut data_types = Vec::with_capacity(data_type_count as usize);
        for _ in 0..data_type_count {
            let data_type = reader.read_u16()?;
            if data_type as usize >= KEY_ITEM_DATA_TYPE_SIZE.len() {
                return Err(corrupt(format!("数据类型异常：{data_type}")));
            }
            data_types.push(data_type);
        }
        let attr_idx = reader.read_i32()?;
        let key_data_idx = reader.read_i32()?;
        let _data_idx = reader.read_i32()?;
        reader.read_u32()?; // v6，未使用
        if attr_idx >= attr_count as i32 || attr_idx < -1 {
            return Err(corrupt(format!("属性索引异常：{attr_idx}")));
        }
        Ok(KeyItem {
            dict_type_def,
            data_types,
            attr_idx,
            key_data_idx,
        })
    }

    fn read_attribute(reader: &mut Reader<'_>) -> Result<AttributeItem, LexiconError> {
        let count = reader.read_i32()?;
        reader.read_u32()?; // a2
        let data_id = reader.read_i32()?;
        reader.read_u32()?; // b2
        if count < 0 {
            return Err(corrupt(format!("属性数量异常：{count}")));
        }
        Ok(AttributeItem { count, data_id })
    }

    fn read_header_item(reader: &mut Reader<'_>) -> Result<HeaderItem, LexiconError> {
        Ok(HeaderItem {
            offset: i64::from(reader.read_u32()?),
            data_size: reader.read_i32()?,
            used_data_size: reader.read_i32()?,
        })
    }

    fn read_all_words(
        reader: &mut Reader<'_>,
        dict: &DictData,
        p2: i64,
        p3: i64,
        size: i64,
    ) -> Result<Vec<RawEntry>, LexiconError> {
        let key = dict.keys.first().ok_or_else(|| corrupt("缺少词库键表"))?;
        let attr_idx = key.attr_idx;
        if attr_idx < 0 || attr_idx as usize >= dict.attributes.len() {
            return Err(corrupt("首个键缺少属性表"));
        }

        let attr_header = dict
            .header_items_attr
            .get(attr_idx as usize)
            .ok_or_else(|| corrupt("属性表索引越界"))?;
        let attr_count = if attr_header.used_data_size == 0 {
            attr_header.data_size
        } else {
            attr_header.used_data_size
        };
        if !(0..=MAX_ATTR_COUNT).contains(&attr_count) {
            return Err(corrupt(format!("属性数量异常：{attr_count}")));
        }

        let hash_store_count = dict.base_hash_size[0];
        if !(1..=MAX_HASH_STORES).contains(&hash_store_count) {
            return Err(corrupt(format!("哈希表数量异常：{hash_store_count}")));
        }
        let data_id = dict.attributes[attr_idx as usize].data_id;
        if data_id < 0 || data_id as usize >= dict.data_store.len() {
            return Err(corrupt(format!("数据表索引越界：{data_id}")));
        }

        let mut entries: Vec<RawEntry> = Vec::new();
        let mut hash_position = dict.hash_store_position(0)?;
        let mut visited = 0i64;

        for _ in 0..hash_store_count {
            reader.seek(hash_position)?;
            let hash_offset = reader.read_i32()?;
            let hash_count = reader.read_i32()?;
            hash_position = reader.position();
            if !(0..=MAX_ENTRIES as i32).contains(&hash_count) {
                return Err(corrupt(format!("哈希项数量异常：{hash_count}")));
            }

            for attr_id in 0..hash_count as i64 {
                let attr_base = dict.attribute_position_from_index(0, attr_id, i64::from(hash_offset))?;
                reader.seek(attr_base + i64::from(dict.data_type_size[0]) - 4)?;
                let mut next_offset = i64::from(reader.read_i32()?);

                for _ in 0..attr_count {
                    visited += 1;
                    if visited > MAX_VISITS {
                        return Err(corrupt("属性链过长"));
                    }
                    let attr2_base = dict.attribute_position_from_attribute(0, next_offset)?;
                    if attr2_base < 0 || attr2_base > size {
                        break;
                    }

                    reader.seek(attr2_base + i64::from(dict.attr_size[attr_idx as usize]) - 4)?;
                    next_offset = i64::from(reader.read_i32()?);
                    entries.push(read_word(reader, dict, attr_base, attr2_base, p2, p3)?);
                    if entries.len() > MAX_ENTRIES {
                        return Err(corrupt("词条数量超过上限"));
                    }
                    if next_offset == -1 {
                        break;
                    }
                }
            }
        }
        Ok(entries)
    }

    fn read_word(
        reader: &mut Reader<'_>,
        dict: &DictData,
        attr_pos1: i64,
        attr_pos2: i64,
        p2: i64,
        p3: i64,
    ) -> Result<RawEntry, LexiconError> {
        let key = &dict.keys[0];
        reader.seek(attr_pos1)?;
        let pinyin_offset = i64::from(reader.read_u32()?);
        let pinyin_position = dict.data_position(key.key_data_idx, pinyin_offset)?;
        let code = decrypt_pinyin(reader, pinyin_position)?;

        reader.seek(attr_pos2)?;
        let offset = i64::from(reader.read_i32()?);
        let frequency = i32::from(reader.read_u16()?);
        reader.read_u16()?; // aFlag
        reader.read_u32()?; // i8
        let p1 = i64::from(reader.read_u16()?);
        reader.read_i32()?; // iE
        reader.skip(4)?; // 下一属性偏移

        let data_id = dict.attributes[key.attr_idx as usize].data_id;
        let word_position = dict.data_position(data_id, offset)?;
        reader.seek(word_position)?;
        let word = decrypt_words(reader, p1, p2, p3)?;
        Ok(RawEntry {
            word,
            code,
            frequency,
        })
    }

    fn decrypt_pinyin(reader: &mut Reader<'_>, position: i64) -> Result<String, LexiconError> {
        reader.seek(position)?;
        let count = reader.read_u16()? / 2;
        if count > MAX_CODE_UNITS {
            return Err(corrupt(format!("拼音数量异常：{count}")));
        }
        let mut indexes = Vec::with_capacity(count as usize);
        for _ in 0..count {
            indexes.push(reader.read_u16()?);
        }
        Ok(syllables_of(&indexes))
    }

    fn decrypt_words(
        reader: &mut Reader<'_>,
        p1: i64,
        p2: i64,
        p3: i64,
    ) -> Result<String, LexiconError> {
        let xk = (((p1 + p2) << 2) + ((p1 + p3) << 2)) & 0xffff;
        let count = reader.read_u16()? / 2;
        if count > MAX_CODE_UNITS {
            return Err(corrupt(format!("词条长度异常：{count}")));
        }
        if !reader.can_read(count as usize * 2) {
            return Err(corrupt("词条数据提前结束"));
        }
        let shift = (p2.rem_euclid(8)) as u32;
        let mut word = String::with_capacity(count as usize);
        for _ in 0..count {
            let ch = u32::from(reader.read_u16()?);
            let rotated = if shift == 0 {
                ch
            } else {
                ((ch << (16 - shift)) | (ch >> shift)) & 0xffff
            };
            let decoded = (rotated as i64 ^ xk) as u32 & 0xffff;
            word.push(char::from_u32(decoded).unwrap_or('\u{fffd}'));
        }
        Ok(word)
    }

    impl DictData {
        fn initialize(&mut self) -> Result<(), LexiconError> {
            let key_hash_size = 500;
            self.data_type_size = vec![0; self.keys.len()];
            self.attr_size = vec![0; self.attributes.len()];
            self.base_hash_size = vec![0; self.keys.len()];

            for index in 0..self.keys.len() {
                let (dict_type_def, attr_idx) = {
                    let key = &self.keys[index];
                    (key.dict_type_def, key.attr_idx)
                };
                let masked_type_def = i32::from(dict_type_def) & TYPE_DEF_MASK;

                let hash_size = if index == 0 && key_hash_size > 0 {
                    key_hash_size
                } else {
                    DATATYPE_HASH_SIZE
                        .get(masked_type_def as usize)
                        .copied()
                        .filter(|value| *value > 0)
                        .ok_or_else(|| corrupt(format!("词库类型异常：{masked_type_def}")))?
                };
                self.base_hash_size[index] = hash_size;

                let mut size = (i32::from(dict_type_def) >> 2) & 4;
                if attr_idx < 0 {
                    let data_types = &self.keys[index].data_types;
                    for (j, data_type) in data_types.iter().enumerate() {
                        if j > 0 || masked_type_def != 4 {
                            size += data_type_size_of(*data_type)?;
                        }
                    }
                    if attr_idx == -1 {
                        size += 4;
                    }
                    self.data_type_size[index] = size;
                } else {
                    let attribute = self
                        .attributes
                        .get(attr_idx as usize)
                        .ok_or_else(|| corrupt("属性表索引越界"))?;
                    let non_attribute_count =
                        self.keys[index].data_types.len() as i32 - attribute.count;
                    if non_attribute_count < 0 {
                        return Err(corrupt("属性数据类型数量异常"));
                    }
                    for j in 0..non_attribute_count as usize {
                        if j > 0 || masked_type_def != 4 {
                            size += data_type_size_of(self.keys[index].data_types[j])?;
                        }
                    }
                    if (dict_type_def & 0x60) > 0 {
                        size += 4;
                    }
                    size += 4;
                    self.data_type_size[index] = size;

                    let mut attribute_size = 0;
                    for data_type in self.keys[index]
                        .data_types
                        .iter()
                        .skip(non_attribute_count as usize)
                    {
                        attribute_size += data_type_size_of(*data_type)?;
                    }
                    if (dict_type_def & 0x40) == 0 {
                        attribute_size += 4;
                    }
                    self.attr_size[attr_idx as usize] = attribute_size;
                }
            }
            Ok(())
        }

        fn hash_store_position(&self, index_id: usize) -> Result<i64, LexiconError> {
            let header = self
                .header_items_idx
                .get(index_id)
                .ok_or_else(|| corrupt(format!("索引表越界：{index_id}")))?;
            Ok(self.ds_base + header.offset - 8 * i64::from(self.base_hash_size[index_id]))
        }

        fn attribute_position_from_index(
            &self,
            index_id: usize,
            attr_id: i64,
            offset: i64,
        ) -> Result<i64, LexiconError> {
            let header = self
                .header_items_idx
                .get(index_id)
                .ok_or_else(|| corrupt(format!("索引表越界：{index_id}")))?;
            Ok(self.ds_base
                + header.offset
                + offset
                + attr_id * i64::from(self.data_type_size[index_id]))
        }

        fn attribute_position_from_attribute(
            &self,
            key_id: usize,
            offset: i64,
        ) -> Result<i64, LexiconError> {
            let key = self
                .keys
                .get(key_id)
                .ok_or_else(|| corrupt(format!("键表越界：{key_id}")))?;
            let header = self
                .header_items_attr
                .get(key.attr_idx as usize)
                .ok_or_else(|| corrupt(format!("属性表越界：{}", key.attr_idx)))?;
            Ok(self.ds_base + header.offset + offset)
        }

        fn data_position(&self, data_id: i32, offset: i64) -> Result<i64, LexiconError> {
            if data_id < 0 {
                return Err(corrupt(format!("数据表越界：{data_id}")));
            }
            let header = self
                .data_store
                .get(data_id as usize)
                .ok_or_else(|| corrupt(format!("数据表越界：{data_id}")))?;
            Ok(self.ds_base + header.offset + offset)
        }
    }

    fn data_type_size_of(data_type: u16) -> Result<i32, LexiconError> {
        KEY_ITEM_DATA_TYPE_SIZE
            .get(data_type as usize)
            .copied()
            .ok_or_else(|| corrupt(format!("数据类型异常：{data_type}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/sgpybin/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture")
    }

    #[test]
    fn parses_new_sgpu_backup() {
        let parsed = SogouBin.parse(&fixture("sogou-new.bin")).expect("parse");
        assert_eq!(parsed.format_id, "sgpybin");
        assert_eq!(parsed.code_system, CodeSystem::Pinyin);
        assert_eq!(parsed.entries.len(), 30);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.duplicates, 0);
        assert_eq!(parsed.entries[0].word, "斌");
        assert_eq!(parsed.entries[0].code, "bin");
        assert_eq!(parsed.entries[0].weight, 1);
        assert_eq!(parsed.entries[2].word, "不通");
        assert_eq!(parsed.entries[2].code, "bu tong");
        assert_eq!(parsed.entries[9].word, "看看");
        assert_eq!(parsed.entries[9].weight, 2);
        assert_eq!(parsed.entries[28].word, "Issue");
        assert_eq!(parsed.entries[28].code, "i s s u e");
        assert_eq!(parsed.entries[28].weight, 3);
        assert_eq!(parsed.entries.last().unwrap().word, "push");
        assert_eq!(parsed.entries.last().unwrap().code, "p u s h");
    }

    #[test]
    fn parses_old_plain_backup() {
        let parsed = SogouBin.parse(&fixture("sogou-old.bin")).expect("parse");
        assert_eq!(parsed.entries.len(), 40);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.duplicates, 0);
        assert_eq!(parsed.entries[0].word, "啊");
        assert_eq!(parsed.entries[0].code, "a");
        assert_eq!(parsed.entries[0].weight, 7);
        assert_eq!(parsed.entries[10].word, "代码");
        assert_eq!(parsed.entries[10].code, "dai ma");
        assert_eq!(parsed.entries[11].word, "的");
        assert_eq!(parsed.entries[11].weight, 3);
        assert_eq!(parsed.entries.last().unwrap().word, "需求加上");
        assert_eq!(parsed.entries.last().unwrap().code, "xu qiu jia shang");
    }

    #[test]
    fn recognizes_both_magics() {
        assert!(SogouBin.looks_like(&fixture("sogou-new.bin")));
        assert!(SogouBin.looks_like(&fixture("sogou-old.bin")));
        assert!(!SogouBin.looks_like(b"nope"));
    }

    #[test]
    fn convert_skips_broken_rows() {
        let raw = vec![
            RawEntry { word: "好".into(), code: "hao".into(), frequency: 0 },
            RawEntry { word: "好".into(), code: "hao".into(), frequency: 5 },
            RawEntry { word: "".into(), code: "hao".into(), frequency: 1 },
            RawEntry { word: "坏".into(), code: "".into(), frequency: 1 },
            RawEntry { word: "行\t列".into(), code: "hang lie".into(), frequency: 1 },
        ];
        let converted = convert(&raw);
        assert_eq!(converted.entries.len(), 1);
        assert_eq!(converted.entries[0].weight, 1);
        assert_eq!(converted.skipped, 3);
        assert_eq!(converted.duplicates, 1);
    }
}
