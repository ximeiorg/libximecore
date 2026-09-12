//! 插件能力声明模型（单一来源：manifest.yaml → Capabilities）。
//!
//! 宿主根据 capabilities 决定：
//! - 注入哪些 host API（quickSend / clipboard / asr / ws）
//! - 是否调用热路径函数（candidate_transform，15ms 硬超时）
//! - 订阅哪些事件（events 列表）

use serde::Deserialize;

/// 插件能力声明（对应 manifest.capabilities）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PluginCapabilities {
    /// Emoji 插件布局与搜索配置。
    #[serde(default)]
    pub emoji: Option<EmojiCapabilities>,
    /// 语音识别插件能力。
    #[serde(default)]
    pub speech: Option<SpeechCapabilities>,
    /// 工具面板插件能力。
    #[serde(default)]
    pub tool: Option<ToolCapabilities>,
    /// 剪贴板同步插件能力。
    #[serde(default)]
    pub clipboard_sync: Option<ClipboardSyncCapabilities>,
    /// 候选词转换热路径（15ms 超时）。
    #[serde(default)]
    pub candidate_transform: bool,
    /// 快捷发送只读 API（host.quickSend）。
    #[serde(default)]
    pub quick_send_read: bool,
    /// 剪贴板只读 API（host.clipboard）。
    #[serde(default)]
    pub clipboard_read: bool,
    /// 订阅的事件列表（如 "input_changed", "text_committed"）。
    #[serde(default)]
    pub events: Vec<String>,
}

/// Emoji 插件能力。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmojiCapabilities {
    /// 是否支持搜索。
    #[serde(rename = "supportsSearch", default)]
    pub supports_search: bool,
    /// 分类列表（可选，静态声明）。
    #[serde(default)]
    pub categories: Vec<String>,
    /// 列数（布局配置）。
    #[serde(default)]
    pub columns: Option<i64>,
    /// 单项高度 dp（布局配置）。
    #[serde(rename = "itemHeightDp", default)]
    pub item_height_dp: Option<i64>,
}

/// 语音识别插件能力。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SpeechCapabilities {
    /// 输入模式："streaming" | "batch"。
    #[serde(rename = "inputMode", default)]
    pub input_mode: String,
    /// 是否支持部分结果（实时回显）。
    #[serde(rename = "supportsPartialResults", default)]
    pub supports_partial_results: bool,
    /// 是否需要网络。
    #[serde(rename = "requiresNetwork", default)]
    pub requires_network: bool,
}

/// 工具面板插件能力。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolCapabilities {
    /// 显示模式："direct" | "passive"。
    #[serde(default)]
    pub display: String,
}

/// 剪贴板同步插件能力。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ClipboardSyncCapabilities {
    /// 支持的协议列表（如 "webdav", "s3", "ximed"）。
    #[serde(default)]
    pub protocols: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_emoji_capabilities() {
        let yaml = r#"
supportsSearch: true
categories:
  - 颜文字
columns: 3
itemHeightDp: 30
"#;
        let cap: EmojiCapabilities = serde_yaml::from_str(yaml).unwrap();
        assert!(cap.supports_search);
        assert_eq!(cap.categories, vec!["颜文字"]);
        assert_eq!(cap.columns, Some(3));
        assert_eq!(cap.item_height_dp, Some(30));
    }

    #[test]
    fn parse_full_capabilities() {
        let yaml = r#"
candidate_transform: true
quick_send_read: true
events:
  - input_changed
  - text_committed
clipboard_sync:
  protocols:
    - webdav
    - ximed
"#;
        let cap: PluginCapabilities = serde_yaml::from_str(yaml).unwrap();
        assert!(cap.candidate_transform);
        assert!(cap.quick_send_read);
        assert_eq!(cap.events, vec!["input_changed", "text_committed"]);
        let cs = cap.clipboard_sync.unwrap();
        assert_eq!(cs.protocols, vec!["webdav", "ximed"]);
    }

    #[test]
    fn parse_speech_capabilities() {
        let yaml = r#"
inputMode: streaming
supportsPartialResults: true
requiresNetwork: true
"#;
        let cap: SpeechCapabilities = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cap.input_mode, "streaming");
        assert!(cap.supports_partial_results);
        assert!(cap.requires_network);
    }

    #[test]
    fn parse_empty_capabilities() {
        let cap: PluginCapabilities = serde_yaml::from_str("").unwrap();
        assert!(!cap.candidate_transform);
        assert!(!cap.quick_send_read);
        assert!(cap.events.is_empty());
    }
}
