//! 标签文件（`char_index.json`）解析。
//!
//! 训练仓的格式是 `{"chars": ["?", "一", ...]}`，下标即类别 id。

use serde::Deserialize;

use crate::InferError;

#[derive(Deserialize)]
struct CharIndex {
    chars: Vec<String>,
}

/// 解析字符表；顺序必须与模型输出类别一一对应。
pub fn parse_char_index(json: &str) -> Result<Vec<String>, InferError> {
    Ok(serde_json::from_str::<CharIndex>(json)?.chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chars() {
        let labels = parse_char_index(r#"{"chars":["?","一","乙"]}"#).unwrap();
        assert_eq!(labels, vec!["?", "一", "乙"]);
    }

    #[test]
    fn rejects_other_shapes() {
        assert!(parse_char_index(r#"["一","乙"]"#).is_err());
        assert!(parse_char_index(r#"{"labels":["一"]}"#).is_err());
    }
}
