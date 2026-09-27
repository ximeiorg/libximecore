use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

use crate::capabilities::PluginCapabilities;

/// manifest 解析错误（manifest.json 优先，兼容旧 manifest.yaml）。
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("读取 manifest 失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("解析 manifest.json 失败: {0}")]
    ParseJson(#[from] serde_json::Error),
    #[error("解析 manifest.yaml 失败: {0}")]
    ParseYaml(#[from] serde_yaml::Error),
    #[error("manifest 缺少 id 字段")]
    MissingId,
    #[error("manifest 缺少 entry 字段")]
    MissingEntry,
}

/// 插件类型（对应 manifest.type）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginType {
    /// 表情（多选启用，即时生效）。
    Emoji,
    /// 语音识别（单选激活）。
    Speech,
    /// 智能联想（预留）。
    Prediction,
    /// 剪贴板同步（单选激活，契约同 Android `clipboard_sync`：push/pull/testConnection）。
    ClipboardSync,
    /// 云备份（单选激活，契约同 Android `backup`：pushBackup/listBackups/
    /// pullBackup/deleteBackup/testConnection；宿主打包，插件只传输）。
    Backup,
    /// 工具面板（单选/多选，面板交互）。
    Tool,
    /// 其他 / 未知。
    Other,
}

/// 插件包根目录的 manifest（.xipk 元数据；manifest.json 优先，兼容旧 manifest.yaml）。
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub version: String,
    /// emoji / speech / prediction / tool / clipboard_sync
    #[serde(rename = "type", default)]
    pub plugin_type: String,
    /// single / multi / none
    #[serde(default)]
    pub activation: String,
    /// 入口脚本（相对插件包根目录），默认 main.js。
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(rename = "minHostVersion", default)]
    pub min_host_version: String,
    #[serde(rename = "maxHostVersion", default)]
    pub max_host_version: String,
    #[serde(rename = "sdkVersion", default)]
    pub sdk_version: String,
    /// 能力声明（结构化解析，宿主据此决定如何消费）。
    #[serde(default)]
    pub capabilities: PluginCapabilities,
    /// 网络访问声明。
    #[serde(default)]
    pub network: NetworkDecl,
    /// 工具栏按钮声明。
    #[serde(rename = "toolbarButtons", default)]
    pub toolbar_buttons: Vec<ToolbarButton>,
}

fn default_entry() -> String {
    "main.js".to_string()
}

/// 网络访问声明（联网域名白名单）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NetworkDecl {
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(rename = "allowCustomHosts", default)]
    pub allow_custom_hosts: bool,
}

/// 工具栏按钮声明。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolbarButton {
    /// 全局唯一 ID（不含逗号）。
    pub id: String,
    /// 按钮文字。
    pub label: String,
    /// 图标（资源文件名）。
    #[serde(default)]
    pub icon: String,
    /// 点击动作（默认 "open_panel"）。
    #[serde(default = "default_action")]
    pub action: String,
}

fn default_action() -> String {
    "open_panel".to_string()
}

impl PluginManifest {
    /// 从 JSON 文本解析并校验必填字段（xipm 工具链产物 / Android 包格式）。
    pub fn parse_json(json: &str) -> Result<Self, ManifestError> {
        let manifest: PluginManifest = serde_json::from_str(json)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// 从 YAML 文本解析并校验必填字段（兼容存量插件包）。
    pub fn parse(yaml: &str) -> Result<Self, ManifestError> {
        let manifest: PluginManifest = serde_yaml::from_str(yaml)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// 解析并校验。id 与 entry 为必填。
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.id.is_empty() {
            return Err(ManifestError::MissingId);
        }
        if self.entry.is_empty() {
            return Err(ManifestError::MissingEntry);
        }
        Ok(())
    }

    pub fn plugin_type(&self) -> PluginType {
        match self.plugin_type.to_lowercase().as_str() {
            "emoji" => PluginType::Emoji,
            "speech" => PluginType::Speech,
            "prediction" => PluginType::Prediction,
            "clipboard_sync" => PluginType::ClipboardSync,
            "backup" => PluginType::Backup,
            "tool" => PluginType::Tool,
            _ => PluginType::Other,
        }
    }

    /// 从已解压的插件目录读取 manifest：manifest.json 优先，回落 manifest.yaml。
    pub fn from_dir(dir: &Path) -> Result<Self, ManifestError> {
        let json_path = dir.join("manifest.json");
        if json_path.is_file() {
            return Self::parse_json(&std::fs::read_to_string(json_path)?);
        }
        Self::parse(&std::fs::read_to_string(dir.join("manifest.yaml"))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
id: com.kingzcheung.xime.plugin.kaomoji
name: 颜文字表情包
description: 包含 174 个常用颜文字的表情插件
icon: "థ౪థ"
version: 2.1.0
type: emoji
activation: multi
entry: main.lua
minHostVersion: 2.6.0
capabilities:
  emoji:
    supportsSearch: true
    categories:
      - 颜文字
network:
  hosts:
    - dashscope.aliyuncs.com
"#;

    #[test]
    fn parse_manifest() {
        let m = PluginManifest::parse(MANIFEST).unwrap();
        assert_eq!(m.id, "com.kingzcheung.xime.plugin.kaomoji");
        assert_eq!(m.version, "2.1.0");
        assert_eq!(m.plugin_type(), PluginType::Emoji);
        assert_eq!(m.entry, "main.lua");
        assert_eq!(m.min_host_version, "2.6.0");
        assert_eq!(m.network.hosts, vec!["dashscope.aliyuncs.com"]);
        let cap = &m.capabilities;
        assert!(cap.emoji.as_ref().unwrap().supports_search);
    }

    #[test]
    fn parse_clipboard_sync_type() {
        let m = PluginManifest::parse(
            "id: com.kingzcheung.xime.plugin.ximed_sync\n\
             name: ximed 剪贴板同步\n\
             version: 0.1.0\n\
             type: clipboard_sync\n\
             activation: single\n",
        )
        .unwrap();
        assert_eq!(m.plugin_type(), PluginType::ClipboardSync);
    }

    #[test]
    fn parse_tool_type() {
        let yaml = r#"id: com.example.tool
name: My Tool
version: 1.0.0
type: tool
activation: single
capabilities:
  tool:
    display: direct
  candidate_transform: true
  events:
    - input_changed
"#;
        let m = PluginManifest::parse(yaml).unwrap();
        assert_eq!(m.plugin_type(), PluginType::Tool);
        assert!(m.capabilities.candidate_transform);
        let tool = m.capabilities.tool.unwrap();
        assert_eq!(tool.display, "direct");
        assert_eq!(m.capabilities.events, vec!["input_changed"]);
    }

    #[test]
    fn parse_toolbar_buttons() {
        let yaml = r#"id: com.example.toolbar
name: Toolbar
version: 1.0.0
type: tool
toolbarButtons:
  - id: btn1
    label: Button 1
    icon: icon.png
    action: open_panel
"#;
        let m = PluginManifest::parse(yaml).unwrap();
        assert_eq!(m.toolbar_buttons.len(), 1);
        assert_eq!(m.toolbar_buttons[0].id, "btn1");
        assert_eq!(m.toolbar_buttons[0].label, "Button 1");
    }

    #[test]
    fn parse_manifest_missing_id() {
        let err = PluginManifest::parse("name: x\nversion: 1.0.0\n").unwrap_err();
        assert!(matches!(err, ManifestError::MissingId));
    }

    #[test]
    fn parse_manifest_missing_entry() {
        let err = PluginManifest::parse("id: a\nname: b\nversion: 1\nentry: ''\n").unwrap_err();
        assert!(matches!(err, ManifestError::MissingEntry));
    }

    #[test]
    fn default_entry_is_main_js() {
        let m = PluginManifest::parse("id: a\nname: b\nversion: 1\n").unwrap();
        assert_eq!(m.entry, "main.js");
        assert_eq!(m.plugin_type(), PluginType::Other);
    }

    #[test]
    fn parse_json_manifest_and_from_dir_prefers_json() {
        let dir = std::env::temp_dir().join(format!("xime_manifest_json_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{
  "id": "com.example.jsplugin",
  "name": "JS 插件",
  "version": "1.0.0",
  "type": "tool",
  "capabilities": {
    "tool": { "display": "direct" },
    "candidate_transform": true,
    "events": ["text_committed"]
  }
}"#,
        )
        .unwrap();

        // entry 缺省 → main.js
        let m = PluginManifest::from_dir(&dir).unwrap();
        assert_eq!(m.id, "com.example.jsplugin");
        assert_eq!(m.entry, "main.js");
        assert_eq!(m.plugin_type(), PluginType::Tool);
        assert!(m.capabilities.candidate_transform);
        assert_eq!(m.capabilities.events, vec!["text_committed"]);

        // 存放 YAML 时回落可读（写入 JSON 后再放 YAML，from_dir 仍优先 JSON）
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: com.example.old\nname: Old\nversion: 0.9.0\n",
        )
        .unwrap();
        assert_eq!(
            PluginManifest::from_dir(&dir).unwrap().id,
            "com.example.jsplugin"
        );

        // 仅 YAML 的存量包
        std::fs::remove_file(dir.join("manifest.json")).unwrap();
        let old = PluginManifest::from_dir(&dir).unwrap();
        assert_eq!(old.id, "com.example.old");
        assert_eq!(old.entry, "main.js", "YAML 缺省 entry 也应回落 main.js");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
