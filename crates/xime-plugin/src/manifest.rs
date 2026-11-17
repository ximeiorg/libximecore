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
    ParseJson(#[from] json5::Error),
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

/// 当前运行平台，取值与 manifest 的 `platforms` 一致：
/// `macos` / `windows` / `linux` / `android` / `ios`。
pub fn current_platform() -> &'static str {
    std::env::consts::OS
}

/// 桌面平台标识（与 `std::env::consts::OS`、`xime.yaml` 的 `metadata.platform` 一致）。
///
/// `platforms` 里要逐个写具体平台，不接受 `pc` 之类的简写。
pub const DESKTOP_PLATFORMS: [&str; 3] = ["macos", "windows", "linux"];

/// 平台是否属于桌面组（macos / windows / linux）。
pub fn is_pc_platform(platform: &str) -> bool {
    DESKTOP_PLATFORMS
        .iter()
        .any(|p| p.eq_ignore_ascii_case(platform))
}

/// 平台列表是否命中目标平台（大小写不敏感；列表里须写具体平台名）。
pub fn platform_matches(platforms: &[String], platform: &str) -> bool {
    platforms
        .iter()
        .any(|p| p.eq_ignore_ascii_case(platform))
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
    /// 宿主最低版本（仅 Android 宿主应用版本）。
    ///
    /// 这是与 Android 共享的字段，Android 侧模型是 `String`（kaml / kotlinx），
    /// 类型不能改，否则整条索引或整包清单在 Android 上反序列化失败。
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
    /// 目标平台，逐个写明：`android` / `ios` / `macos` / `windows` / `linux`。
    ///
    /// 声明了就按列表匹配；未声明时 [`PluginManifest::supports_platform`] 按
    /// Android 契约视为 `["android"]`，而安装门禁
    /// [`PluginManifest::runs_on_platform`] 放行（存量包兼容）。
    #[serde(default)]
    pub platforms: Vec<String>,
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
    /// 对齐 Android kotlinx 宽松模式：容忍 `//`/`/* */` 注释与尾逗号（JSON5 超集）。
    pub fn parse_json(json: &str) -> Result<Self, ManifestError> {
        let manifest: PluginManifest = json5::from_str(json)?;
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

    /// 平台门禁：目标平台是否在声明列表内（大小写不敏感）。
    /// 缺省 `platforms` 视为 `["android"]`（同 Android 契约；宿主自行决定是否启用门禁）。
    pub fn supports_platform(&self, platform: &str) -> bool {
        if self.platforms.is_empty() {
            platform == "android"
        } else {
            platform_matches(&self.platforms, platform)
        }
    }

    /// 安装门禁用的平台判断：未声明 `platforms` 的存量包一律放行，
    /// 声明了才按列表匹配。
    pub fn runs_on_platform(&self, platform: &str) -> bool {
        self.platforms.is_empty() || platform_matches(&self.platforms, platform)
    }

    /// 当前平台是否可安装 / 运行。
    pub fn runs_on_current_platform(&self) -> bool {
        self.runs_on_platform(current_platform())
    }

    /// 给定宿主版本是否满足包内声明的 `minHostVersion` / `maxHostVersion`。
    ///
    /// **桌面端安装流程不调用它**：那两个字段是 Android 应用版本（3.x），桌面三端
    /// 各有自己的应用版本（0.1.0 / 0.3.1 / …），没有可共用的下限。宿主若要自行
    /// 校验（例如某个发行版有额外要求），再显式调用并自行决定传什么版本。
    pub fn host_compatible(&self, host_version: &str) -> bool {
        version_compatible(
            host_version,
            &self.min_host_version,
            &self.max_host_version,
        )
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

/// 版本段解析（同 Android `VersionUtil`：按 `.` `-` `+` 切分，仅保留纯数字段，
/// 最多 4 段，缺失补 0；预发布后缀被忽略，如 `2.6.0-beta3` ≈ `2.6.0`）。
fn version_segments(version: &str) -> [u64; 4] {
    let mut segments = [0u64; 4];
    let mut idx = 0;
    for part in version.split(['.', '-', '+']) {
        if idx >= segments.len() {
            break;
        }
        if !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()) {
            segments[idx] = part.parse().unwrap_or(u64::MAX);
            idx += 1;
        }
    }
    segments
}

/// 比较两个版本号（返回正/零/负；同 Android `VersionUtil.compare` 语义）。
pub(crate) fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    version_segments(a).cmp(&version_segments(b))
}

/// 宿主版本是否满足插件的 `minHostVersion`/`maxHostVersion` 区间（空 = 不限制）。
pub(crate) fn version_compatible(host: &str, min: &str, max: &str) -> bool {
    if !min.is_empty() && compare_versions(host, min) == std::cmp::Ordering::Less {
        return false;
    }
    if !max.is_empty() && compare_versions(host, max) == std::cmp::Ordering::Greater {
        return false;
    }
    true
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
    fn parse_json_lenient_comments_and_trailing_commas() {
        // Android xipm 产物形态：行注释 + 块注释 + 尾逗号
        let json = r#"{
  // 插件唯一标识
  "id": "com.example.lenient",
  "name": "宽松解析",
  "version": "1.0.0", // 尾注释
  "type": "emoji",
  "platforms": ["android", "macos",],
  /* 块注释 */
  "network": {
    "hosts": [
      "api.example.org", /* 内联块注释 */ "cdn.example.org",
    ],
    "allowCustomHosts": true,
  },
}"#;
        let m = PluginManifest::parse_json(json).unwrap();
        assert_eq!(m.id, "com.example.lenient");
        assert_eq!(m.plugin_type(), PluginType::Emoji);
        assert_eq!(
            m.network.hosts,
            vec!["api.example.org".to_string(), "cdn.example.org".to_string()]
        );
        assert!(m.network.allow_custom_hosts);
        assert!(m.supports_platform("macos"));

        // 字符串内的 //（URL scheme）不受注释剥离影响
        let url_json = r#"{
  // 服务器地址
  "id": "com.example.url",
  "name": "https://dav.example.com/dav/",
}"#;
        let m = PluginManifest::parse_json(url_json).unwrap();
        assert_eq!(m.name, "https://dav.example.com/dav/");
    }

    #[test]
    fn default_entry_is_main_js() {
        let m = PluginManifest::parse("id: a\nname: b\nversion: 1\n").unwrap();
        assert_eq!(m.entry, "main.js");
        assert_eq!(m.plugin_type(), PluginType::Other);
    }

    #[test]
    fn platforms_parse_and_gate() {
        let m = PluginManifest::parse_json(
            r#"{"id":"a","name":"b","version":"1","platforms":["Android","MacOS"]}"#,
        )
        .unwrap();
        assert!(m.supports_platform("android"));
        assert!(m.supports_platform("macos"));
        assert!(!m.supports_platform("ios"));

        // 缺省视为 ["android"]（同 Android 契约）；但安装门禁对存量包放行
        let m = PluginManifest::parse("id: a\nname: b\nversion: 1\n").unwrap();
        assert!(m.supports_platform("android"));
        assert!(!m.supports_platform("macos"));
        assert!(m.runs_on_platform("macos"), "未声明 platforms 的存量包应放行");
        assert!(m.runs_on_platform("android"));
    }

    /// `platforms` 必须逐个写具体平台；大小写不敏感，但不接受 `pc` 简写。
    #[test]
    fn platforms_are_explicit() {
        let m = PluginManifest::parse(
            "id: a\nname: b\nversion: 1\nplatforms: [android, macOS, windows, linux]\n",
        )
        .unwrap();
        assert!(m.supports_platform("android"));
        assert!(m.supports_platform("macos"), "平台名大小写不敏感");
        assert!(m.supports_platform("windows"));
        assert!(m.supports_platform("linux"));
        assert!(!m.supports_platform("ios"));

        // 只支持 Android 的包，在桌面端拒绝安装
        let android_only = PluginManifest::parse(
            "id: a\nname: b\nversion: 1\nplatforms: [android]\n",
        )
        .unwrap();
        assert!(!android_only.runs_on_platform("macos"));
        assert!(android_only.runs_on_platform("android"));

        // `pc` 不是合法平台名，写了也不生效（必须逐个列出）
        let pc_shorthand = PluginManifest::parse(
            "id: a\nname: b\nversion: 1\nplatforms: [android, pc]\n",
        )
        .unwrap();
        assert!(!pc_shorthand.runs_on_platform("macos"));
    }

    /// `minHostVersion` 只描述 Android 宿主应用版本；桌面三端版本各不相同，
    /// 因此安装流程不校验它，需要时宿主自己用 `host_compatible` 判断。
    #[test]
    fn host_version_is_android_only_and_opt_in() {
        let yaml = r#"id: a
name: b
version: 1
platforms: [android, macos, windows, linux]
minHostVersion: '3.0.0'
maxHostVersion: '3.9.9'
"#;
        let m = PluginManifest::parse(yaml).unwrap();
        // 字段本身照常解析（Android 侧语义不变）
        assert_eq!(m.min_host_version, "3.0.0");
        assert_eq!(m.max_host_version, "3.9.9");
        // 桌面端不会因为 Android 的 3.0.0 被挡：安装门禁只看平台
        assert!(m.runs_on_platform("macos"));
        // 宿主若想自行校验，可以显式调；判断规则与 Android 一致（>= min 且 <= max）
        assert!(!m.host_compatible("0.1.0"));
        assert!(m.host_compatible("3.0.0"));
        assert!(!m.host_compatible("3.9.9.1"));
    }

    #[test]
    fn version_utils_match_android_semantics() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("3.0.0", "2.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("2.6.0", "2.6.0"), Ordering::Equal);
        // 预发布后缀被忽略
        assert_eq!(compare_versions("2.6.0-beta3", "2.6.0"), Ordering::Equal);
        // 段缺失补 0
        assert_eq!(compare_versions("3", "3.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("3.0.1", "3"), Ordering::Greater);
        // 非数字段忽略
        assert_eq!(compare_versions("3.0.0.rc1", "3.0.0"), Ordering::Equal);

        assert!(version_compatible("3.0.0", "3.0.0", ""));
        assert!(!version_compatible("3.0.0", "3.1.0", ""));
        assert!(!version_compatible("3.0.0", "", "2.9.9"));
        assert!(version_compatible("3.0.0", "", ""));
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
