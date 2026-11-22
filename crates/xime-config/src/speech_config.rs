//! 语音输入的共享配置（`speech.toml`，位于 Rime 用户目录的**同级**）。
//!
//! **唯一实现**：设置程序（`xime-setup`）与各平台输入法（XimeYi / Windows /
//! Linux 的宿主）都经这里读写同一份文件，谁都不许自己解析一份——剪贴板库曾因
//! 两侧各写一份 schema 出过「界面恒空」的事故（XimeYi DECISIONS D21），同理。
//!
//! # 文件位置
//!
//! 与 `xime.db` / `clipboard.db` 同级（`<用户数据目录的上一级>/speech.toml`），
//! 由 [`crate::rime_deploy::get_data_dirs`] 解析——也就是 `set_rime_paths`
//! 注入的那份路径。输入法与设置程序必须解析到同一个文件，否则「设置里关了、
//! 输入法还照录」。
//!
//! # 兼容
//!
//! 字段**只增不改**：老配置（没有 `enabled` 键）必须按「开启」读，否则升级后
//! 语音会毫无征兆地消失；新配置写全字段，老版本读到陌生键会忽略。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// 配置文件名（配置目录根，与 `clipboard_sync.toml` 同层）。
pub const CONFIG_FILE: &str = "speech.toml";

/// 语音输入用哪个引擎。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechMode {
    /// 本地模型：权重在本机，离线。
    #[default]
    Local,
    /// 在线模型：ASR 插件调云端 API。
    Online,
}

impl SpeechMode {
    /// 分段选择器的文案，顺序与 [`Self::index`] 一致。
    pub const LABELS: [&'static str; 2] = ["本地模型", "在线模型"];

    pub fn index(self) -> usize {
        match self {
            Self::Local => 0,
            Self::Online => 1,
        }
    }

    /// 越界索引一律落到「本地」：索引来自界面，不值得为它引错误态。
    pub fn from_index(i: usize) -> Self {
        if i == 1 {
            Self::Online
        } else {
            Self::Local
        }
    }

    /// 一句话说明（分段下面那行，告诉用户两种模式的差别）。
    pub fn hint(self) -> &'static str {
        match self {
            Self::Local => "模型下载到本机，离线运行，语音不出电脑",
            Self::Online => "由扩展商店的语音插件调用云端 API，需要联网，按插件表单配置",
        }
    }
}

/// 识别完成后怎么上屏。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitMode {
    /// 识别完直接上屏（说完即所得，适合短句）。
    #[default]
    Direct,
    /// 先进候选栏，确认后再上屏（长句/需要改字时更稳）。
    Candidate,
}

impl CommitMode {
    pub const LABELS: [&'static str; 2] = ["直接上屏", "先进候选"];

    pub fn index(self) -> usize {
        match self {
            Self::Direct => 0,
            Self::Candidate => 1,
        }
    }

    pub fn from_index(i: usize) -> Self {
        if i == 1 {
            Self::Candidate
        } else {
            Self::Direct
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Direct => "说完停顿后直接输入到当前窗口",
            Self::Candidate => "识别结果进候选栏，按空格确认、Esc 丢弃",
        }
    }
}

/// 语种选项：`(配置值, 下拉文案)`。本地引擎与在线插件共用这一套取值。
pub const LANGUAGES: [(&str, &str); 4] = [
    ("auto", "自动识别"),
    ("zh", "中文"),
    ("en", "English"),
    ("yue", "粤语"),
];

/// 配置值 → 下拉文案（未知值按自动识别显示，避免界面出现空选项）。
pub fn language_label(code: &str) -> &'static str {
    LANGUAGES
        .iter()
        .find(|(value, _)| *value == code)
        .map(|(_, label)| *label)
        .unwrap_or("自动识别")
}

/// `speech.toml`：总开关 + 模式 + 当前插件 + 通用项。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeechConfig {
    /// 语音输入总开关。`false` = 一键关闭：输入法不再因任何触发（快捷键、
    /// 菜单项、候选栏 🎙️）开录音，设置页也不再拉起语音服务。
    pub enabled: bool,
    /// 本地 / 在线（二选一）。
    pub mode: SpeechMode,
    /// 在线模式下选中的插件 id；空 = 还没选。
    pub plugin_id: String,
    /// 语种：`auto` / `zh` / `en` / `yue`。
    pub language: String,
    /// 上屏方式。
    pub commit: CommitMode,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            // 默认开启：新装用户打开语音页就能用；关闭是显式动作。
            enabled: true,
            mode: SpeechMode::Local,
            plugin_id: String::new(),
            language: "auto".to_string(),
            commit: CommitMode::Direct,
        }
    }
}

impl SpeechConfig {
    /// 修掉手写配置里的越界值：界面不能显示一个不存在的选项。
    pub fn normalized(mut self) -> Self {
        if !LANGUAGES.iter().any(|(code, _)| *code == self.language) {
            self.language = "auto".to_string();
        }
        self.plugin_id = self.plugin_id.trim().to_string();
        self
    }
}

/// `speech.toml` 路径（配置目录根）。
pub fn config_path() -> PathBuf {
    config_path_beside(&crate::rime_deploy::get_data_dirs().1)
}

/// 给定 Rime 用户目录，算出配置文件路径（测试用临时目录也走这里）。
pub fn config_path_beside(user_dir: &Path) -> PathBuf {
    user_dir.parent().unwrap_or(user_dir).join(CONFIG_FILE)
}

/// 读配置；文件不存在或损坏时回落默认值（本地模型、开启）。
pub fn read_config() -> SpeechConfig {
    read_config_at(&config_path())
}

/// 读指定路径的配置（测试 / 多实例用）。
pub fn read_config_at(path: &Path) -> SpeechConfig {
    let Ok(content) = std::fs::read_to_string(path) else {
        return SpeechConfig::default();
    };
    toml::from_str::<SpeechConfig>(&content)
        .unwrap_or_default()
        .normalized()
}

/// 写配置（宿主按 mtime 热加载，切换即时生效）。
pub fn write_config(cfg: &SpeechConfig) {
    write_config_at(&config_path(), cfg);
}

/// 写指定路径的配置（测试用）。
pub fn write_config_at(path: &Path, cfg: &SpeechConfig) {
    let Ok(content) = toml::to_string(cfg) else {
        return;
    };
    // 原子写：输入法侧可能正在同一个文件上做「读-改-写」（关开关时），
    // 半截文件会被当成损坏读回默认值（= 开关莫名其妙又打开）。
    let _ = crate::atomic_write(path, content.as_bytes());
}

/// 读-改-写总开关，其余字段原样保留。
///
/// 返回落盘后的值（写失败时也会返回期望值，调用方按需再读一次核对）。
pub fn set_enabled(enabled: bool) -> bool {
    let path = config_path();
    let mut cfg = read_config_at(&path);
    cfg.enabled = enabled;
    write_config_at(&path, &cfg);
    enabled
}

/// 语音输入是否开启（**热路径**：输入法按键、会话轮询都会调）。
///
/// 只 stat 文件头，内容变了才重解析；缓存键是 `(mtime, len)`，设置程序一落盘
/// 输入法下一拍就能看到，不需要重启或额外通知。
pub fn is_enabled() -> bool {
    enabled_at(&config_path())
}

/// [`is_enabled`] 的指定路径版本（测试用；同样带缓存，按路径各一份）。
fn enabled_at(path: &Path) -> bool {
    struct Cache {
        path: PathBuf,
        stamp: Option<(Option<SystemTime>, u64)>,
        value: bool,
    }
    static CACHE: OnceLock<Mutex<Option<Cache>>> = OnceLock::new();

    let stamp = std::fs::metadata(path)
        .ok()
        .map(|m| (m.modified().ok(), m.len()));
    let mut guard = CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(cache) = guard.as_ref() {
        if cache.path == path && cache.stamp == stamp {
            return cache.value;
        }
    }

    let value = read_config_at(path).enabled;
    *guard = Some(Cache {
        path: path.to_path_buf(),
        stamp,
        value,
    });
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试各用一份临时目录：`speech.toml` 与统一库同级，不能共用系统临时目录。
    fn temp_config() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let user_dir = dir.path().join("rime");
        std::fs::create_dir_all(&user_dir).unwrap();
        let path = config_path_beside(&user_dir);
        (dir, path)
    }

    /// 没有配置文件时默认开启：不能因为「读不到」就把功能关掉。
    #[test]
    fn missing_file_means_enabled() {
        let (_dir, path) = temp_config();
        assert!(!path.exists());
        assert!(read_config_at(&path).enabled);
        assert!(enabled_at(&path));
    }

    /// 老配置（升级前写的，没有 `enabled` 键）必须按开启读。
    #[test]
    fn legacy_config_without_enabled_key_is_enabled() {
        let (_dir, path) = temp_config();
        std::fs::write(
            &path,
            "mode = \"online\"\nplugin_id = \"acme\"\nlanguage = \"zh\"\ncommit = \"candidate\"\n",
        )
        .unwrap();
        let cfg = read_config_at(&path);
        assert!(cfg.enabled);
        assert_eq!(cfg.mode, SpeechMode::Online);
        assert_eq!(cfg.language, "zh");
        assert_eq!(cfg.commit, CommitMode::Candidate);
        assert!(enabled_at(&path));
    }

    /// 关掉再读回来仍是关；其余字段不能丢（读-改-写只该动一个键）。
    #[test]
    fn set_enabled_keeps_other_fields() {
        let (_dir, path) = temp_config();
        let cfg = SpeechConfig {
            mode: SpeechMode::Online,
            plugin_id: "acme".to_string(),
            language: "yue".to_string(),
            commit: CommitMode::Candidate,
            ..SpeechConfig::default()
        };
        write_config_at(&path, &cfg);

        let off = SpeechConfig {
            enabled: false,
            ..read_config_at(&path)
        };
        write_config_at(&path, &off);

        let back = read_config_at(&path);
        assert!(!back.enabled);
        assert_eq!(back.mode, SpeechMode::Online);
        assert_eq!(back.plugin_id, "acme");
        assert_eq!(back.language, "yue");
        assert_eq!(back.commit, CommitMode::Candidate);
        assert!(!enabled_at(&path));
    }

    /// 热路径缓存必须跟着文件走：改一次文件，下一次读就要看到新值。
    #[test]
    fn cached_read_follows_file_changes() {
        let (_dir, path) = temp_config();
        assert!(enabled_at(&path));

        let mut cfg = SpeechConfig::default();
        assert!(cfg.enabled);
        cfg.enabled = false;
        write_config_at(&path, &cfg);
        assert!(!enabled_at(&path), "关闭后必须立即读到 false");

        cfg.enabled = true;
        write_config_at(&path, &cfg);
        assert!(enabled_at(&path), "重新打开后必须立即读到 true");
    }

    /// 损坏的文件不能把功能卡死：按默认（开启）处理，且不 panic。
    #[test]
    fn broken_file_falls_back_to_default() {
        let (_dir, path) = temp_config();
        std::fs::write(&path, "这不是 toml :::").unwrap();
        assert!(read_config_at(&path).enabled);
    }

    /// 路径必须与 xime.db 同级：两边解析到同一个文件是这套开关的前提。
    #[test]
    fn config_path_sits_beside_database() {
        let dir = tempfile::tempdir().unwrap();
        let user_dir = dir.path().join("rime");
        let path = config_path_beside(&user_dir);
        assert_eq!(path, dir.path().join(CONFIG_FILE));
        assert_eq!(
            path.parent(),
            crate::app_db::db_path_beside(&user_dir).parent()
        );
    }
}
