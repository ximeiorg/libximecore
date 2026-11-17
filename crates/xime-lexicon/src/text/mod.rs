//! 文本词库解析（`.txt`）：libime / 搜狗 / 百度三家都用这个扩展名。
//!
//! 三者的列结构、编码、拼音分隔符都不一样，所以各自实现 `looks_like`
//! 让注册表按内容挑格式。

pub(crate) mod baidu_pinyin;
pub(crate) mod libime;
pub(crate) mod sogou_pinyin;
