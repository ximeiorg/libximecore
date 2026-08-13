use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

/// manifest.yaml 解析错误。
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("读取 manifest.yaml 失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("解析 manifest.yaml 失败: {0}")]
    Parse(#[from] serde_yaml::Error),
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
    /// 其他 / 未知。
    Other,
}

/// 插件包根目录的 manifest.yaml（.xipk 元数据）。
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
    /// emoji / speech / prediction ...
    #[serde(rename = "type", default)]
    pub plugin_type: String,
    /// single / multi / none
    #[serde(default)]
    pub activation: String,
    /// 入口脚本（相对插件包根目录），默认 main.lua。
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(rename = "minHostVersion", default)]
    pub min_host_version: String,
    #[serde(rename = "sdkVersion", default)]
    pub sdk_version: String,
    /// 能力声明（宿主据此决定如何消费，宽松解析）。
    #[serde(default)]
    pub capabilities: serde_yaml::Value,
    /// 配置字段声明（宿主渲染表单，宽松解析）。
    #[serde(default)]
    pub config_schema: serde_yaml::Value,
    /// 网络访问声明。
    #[serde(default)]
    pub network: NetworkDecl,
}

fn default_entry() -> String {
    "main.lua".to_string()
}

/// 网络访问声明（联网域名白名单）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NetworkDecl {
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(rename = "allowCustomHosts", default)]
    pub allow_custom_hosts: bool,
}

impl PluginManifest {
    /// 从 YAML 文本解析并校验必填字段。
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
            _ => PluginType::Other,
        }
    }

    /// 从已解压的插件目录读取 manifest.yaml。
    pub fn from_dir(dir: &Path) -> Result<Self, ManifestError> {
        let yaml = std::fs::read_to_string(dir.join("manifest.yaml"))?;
        Self::parse(&yaml)
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
configSchema: []
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
        assert_eq!(
            m.capabilities["emoji"]["supportsSearch"],
            serde_yaml::Value::Bool(true)
        );
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
    fn default_entry_is_main_lua() {
        let m = PluginManifest::parse("id: a\nname: b\nversion: 1\n").unwrap();
        assert_eq!(m.entry, "main.lua");
        assert_eq!(m.plugin_type(), PluginType::Other);
    }
}
