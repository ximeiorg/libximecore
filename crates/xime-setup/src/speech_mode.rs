//! 语音输入的模式与在线插件状态。
//!
//! **二选一的产品结构**（同一时刻只有一个引擎在跑）：
//!
//! - **本地模型**：权重在本机（扩展商店下载或用户导入），离线推理，音频不出电脑；
//! - **在线模型**：扩展商店的 `type: speech` 插件，插件自己调云端 API，配置项由
//!   插件的 `settings.schema` 决定，这里只负责渲染表单与落盘。
//!
//! 模式写进 `speech.toml`，输入法进程读同一份配置并热加载——设置端不直接持有
//! 引擎，只负责「选谁 + 配什么」。
//!
//! 在线插件的配置值走 `plugins/config/<id>.yaml`（与 host.config 同一份密文格式），
//! 与插件运行时读到的完全一致。

#![cfg(feature = "voice-page")]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use xime_plugin::{PluginManifest, PluginType, SettingField};

use crate::state::{
    config_base_dir, load_plugin_runtime, notify_daemon_reload_plugins, read_plugin_config,
    write_plugin_config,
};

/// 操作提示的存活时间（与本地模型页一致，别做两套节奏）。
const MESSAGE_TTL: Duration = Duration::from_secs(5);

/// 语音输入用哪个引擎。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// `speech.toml`：模式 + 当前插件 + 通用项。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SpeechConfig {
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

/// `speech.toml` 路径（配置目录根，与 clipboard_sync.toml 同层）。
pub fn config_path() -> PathBuf {
    config_base_dir().join("speech.toml")
}

/// 读配置；文件不存在或损坏时回落默认值（本地模型）。
pub fn read_config() -> SpeechConfig {
    let Ok(content) = std::fs::read_to_string(config_path()) else {
        return SpeechConfig::default();
    };
    toml::from_str::<SpeechConfig>(&content)
        .unwrap_or_default()
        .normalized()
}

/// 写配置（宿主按 mtime 热加载，切换即时生效）。
pub fn write_config(cfg: &SpeechConfig) {
    if let Ok(content) = toml::to_string(cfg) {
        if let Some(parent) = config_path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(config_path(), content);
    }
}

/// 一个可选的在线语音插件。
#[derive(Clone, Debug, PartialEq)]
pub struct SpeechPluginInfo {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    /// 插件是否启用（停用的插件不该被选中）。
    pub enabled: bool,
}

/// 扫描已安装的 `type: speech` 插件（含未启用，选择时自动启用）。
pub fn scan_speech_plugins() -> Vec<SpeechPluginInfo> {
    scan_speech_plugins_in(&config_base_dir().join("plugins"))
}

/// 可注入根目录的扫描（测试用，不碰真实配置目录）。
pub fn scan_speech_plugins_in(root: &Path) -> Vec<SpeechPluginInfo> {
    let manager = xime_plugin::PluginManager::new(root);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let Ok(manifest) = PluginManifest::from_dir(&dir) else {
            continue;
        };
        if manifest.plugin_type() != PluginType::Speech {
            continue;
        }
        let enabled = manager.get(&manifest.id).map(|r| r.enabled).unwrap_or(true);
        out.push(SpeechPluginInfo {
            id: manifest.id,
            name: manifest.name,
            dir,
            enabled,
        });
    }
    out
}

/// 后台线程的完成结果（schema 加载 / 测试连接）。
enum Outcome {
    Schema {
        fields: Vec<SettingField>,
        values: Vec<(String, String)>,
    },
    Tested,
}

static OUTCOME: Mutex<Option<Result<Outcome, String>>> = Mutex::new(None);

/// 语音输入页的在线侧状态（本地侧在 [`crate::speech_models::SpeechModelState`]）。
#[derive(Clone, Debug)]
pub struct SpeechUiState {
    pub mode: SpeechMode,
    /// 已安装的 speech 插件（顺序稳定）。
    pub plugins: Vec<SpeechPluginInfo>,
    /// 当前选中的插件下标。
    pub plugin_index: usize,
    /// 插件表单（schema + 当前值）。
    pub fields: Vec<(SettingField, String)>,
    pub language: String,
    pub commit: CommitMode,
    /// 进行中的后台操作：`schema` / `test`。
    pub busy: Option<&'static str>,
    message: Option<String>,
    message_at: Option<Instant>,
}

impl Default for SpeechUiState {
    fn default() -> Self {
        let cfg = read_config();
        let plugins = scan_speech_plugins();
        let plugin_index = plugins
            .iter()
            .position(|p| p.id == cfg.plugin_id)
            .unwrap_or(0);
        let mut state = Self {
            mode: cfg.mode,
            plugins,
            plugin_index,
            fields: Vec::new(),
            language: cfg.language,
            commit: cfg.commit,
            busy: None,
            message: None,
            message_at: None,
        };
        // 在线模式但配置里没记住插件（首次切换、或插件被卸载后重装）：
        // 落到第一个可用插件并写回，别让输入法读到「mode=online 但 plugin_id 为空」。
        if state.mode == SpeechMode::Online && state.plugin_id_missing() {
            if let Some(id) = state.plugins.first().map(|p| p.id.clone()) {
                state.persist_plugin(&id);
            }
        }
        // 在线模式且已有插件：进页面就把表单拉出来，别让用户先点一下才看到。
        if state.mode == SpeechMode::Online && state.selected_plugin().is_some() {
            state.start_schema_load();
        }
        state
    }
}

impl SpeechUiState {
    /// 当前选中的插件（没有安装任何 speech 插件时为 None）。
    pub fn selected_plugin(&self) -> Option<&SpeechPluginInfo> {
        self.plugins.get(self.plugin_index)
    }

    /// 在线模式是否可用（装没装插件）。
    pub fn online_available(&self) -> bool {
        !self.plugins.is_empty()
    }

    /// `speech.toml` 里的 plugin_id 与当前列表对不上（空、或被卸载）。
    fn plugin_id_missing(&self) -> bool {
        let cfg = read_config();
        !self.plugins.iter().any(|p| p.id == cfg.plugin_id)
    }

    /// 操作提示（5 秒内有效）。
    pub fn message(&self) -> Option<&str> {
        let fresh = self
            .message_at
            .map(|at| at.elapsed() < MESSAGE_TTL)
            .unwrap_or(false);
        if fresh {
            self.message.as_deref()
        } else {
            None
        }
    }

    fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
        self.message_at = Some(Instant::now());
    }

    /// 切换本地 / 在线并落盘。
    pub fn set_mode(&mut self, mode: SpeechMode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        let mut cfg = read_config();
        cfg.mode = mode;
        write_config(&cfg);

        match mode {
            SpeechMode::Local => {
                self.set_message("已切换为本地模型：语音完全离线处理");
            }
            SpeechMode::Online => {
                if self.plugins.is_empty() {
                    self.set_message("还没有安装语音插件：先去「扩展商店」装一个在线语音插件");
                } else {
                    let plugin = self.plugins[self.plugin_index].clone();
                    self.persist_plugin(&plugin.id);
                    self.enable_plugin(&plugin);
                    self.start_schema_load();
                    self.set_message(format!("已切换为在线模型：{}", plugin.name));
                }
            }
        }
    }

    /// 选择在线插件：写配置、启用插件、加载它的配置表单。
    pub fn select_plugin(&mut self, index: usize) {
        if index >= self.plugins.len() || index == self.plugin_index {
            return;
        }
        self.plugin_index = index;
        let plugin = self.plugins[index].clone();
        self.persist_plugin(&plugin.id);
        self.enable_plugin(&plugin);
        self.fields.clear();
        self.start_schema_load();
        self.set_message(format!("已选择插件：{}", plugin.name));
    }

    /// 编辑一个插件配置项（逐键落盘，与剪贴板/备份页一致）。
    pub fn set_field(&mut self, key: String, value: String) {
        let Some(plugin) = self.selected_plugin().cloned() else {
            return;
        };
        for (field, current) in self.fields.iter_mut() {
            if field.key == key {
                *current = value.clone();
            }
        }
        let mut config = read_plugin_config(&plugin.id);
        config.insert(key, value);
        write_plugin_config(&plugin.id, &config);
    }

    /// 语种（本地与在线共用）。
    pub fn set_language(&mut self, language: String) {
        self.language = language.clone();
        let mut cfg = read_config();
        cfg.language = language;
        write_config(&cfg);
    }

    /// 上屏方式。
    pub fn set_commit(&mut self, commit: CommitMode) {
        self.commit = commit;
        let mut cfg = read_config();
        cfg.commit = commit;
        write_config(&cfg);
    }

    /// 测试当前插件的连接（插件 `test` 槽）。
    pub fn start_test(&mut self) {
        let Some(plugin) = self.selected_plugin().cloned() else {
            self.set_message("先选择一个语音插件");
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("test");
        std::thread::spawn(move || {
            let result = load_plugin_runtime(&plugin.dir, &plugin.id).and_then(|rt| {
                match rt.test_connection() {
                    Some(error) => Err(error),
                    None => Ok(Outcome::Tested),
                }
            });
            *OUTCOME.lock().unwrap() = Some(result);
        });
    }

    /// 后台加载当前插件的 `settings.schema` 与已保存的值。
    fn start_schema_load(&mut self) {
        let Some(plugin) = self.selected_plugin().cloned() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("schema");
        self.fields.clear();
        std::thread::spawn(move || {
            let result = load_plugin_runtime(&plugin.dir, &plugin.id).map(|rt| {
                let fields = rt.get_settings_schema();
                let config = read_plugin_config(&plugin.id);
                let values = fields
                    .iter()
                    .map(|f| {
                        (
                            f.key.clone(),
                            config.get(&f.key).cloned().unwrap_or_default(),
                        )
                    })
                    .collect();
                Outcome::Schema { fields, values }
            });
            *OUTCOME.lock().unwrap() = Some(result);
        });
    }

    /// 后台结果轮询（挂在设置页的 250ms tick 上）。
    pub fn poll(&mut self) {
        let outcome = OUTCOME.lock().unwrap().take();
        let Some(outcome) = outcome else {
            return;
        };
        self.busy = None;
        match outcome {
            Ok(Outcome::Schema { fields, values }) => {
                // schema 与值按 key 对齐；插件改了字段顺序也不会错位。
                let map: std::collections::HashMap<String, String> = values.into_iter().collect();
                self.fields = fields
                    .into_iter()
                    .map(|field| {
                        let value = map.get(&field.key).cloned().unwrap_or_default();
                        (field, value)
                    })
                    .collect();
                if self.fields.is_empty() {
                    self.set_message("该插件没有可配置项");
                }
            }
            Ok(Outcome::Tested) => self.set_message("连接成功"),
            Err(error) => self.set_message(error),
        }
    }

    fn persist_plugin(&mut self, id: &str) {
        let mut cfg = read_config();
        cfg.plugin_id = id.to_string();
        // 选插件即进入在线模式，省得用户再点一次模式。
        cfg.mode = SpeechMode::Online;
        write_config(&cfg);
    }

    fn enable_plugin(&mut self, plugin: &SpeechPluginInfo) {
        if plugin.enabled {
            return;
        }
        let manager = xime_plugin::PluginManager::new(config_base_dir().join("plugins"));
        if let Err(error) = manager.set_enabled(&plugin.id, true) {
            self.set_message(format!("启用插件失败：{error}"));
            return;
        }
        if let Some(slot) = self.plugins.iter_mut().find(|p| p.id == plugin.id) {
            slot.enabled = true;
        }
        notify_daemon_reload_plugins();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_roundtrips_through_toml() {
        let cfg = SpeechConfig {
            mode: SpeechMode::Online,
            plugin_id: "com.example.asr".into(),
            language: "zh".into(),
            commit: CommitMode::Candidate,
        };
        let text = toml::to_string(&cfg).unwrap();
        let back: SpeechConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
        // 序列化用 snake_case：配置是给宿主读的，别用 Rust 的变体名。
        assert!(text.contains("mode = \"online\""), "{text}");
        assert!(text.contains("commit = \"candidate\""), "{text}");
    }

    #[test]
    fn default_is_local_and_sane() {
        let cfg = SpeechConfig::default();
        assert_eq!(cfg.mode, SpeechMode::Local);
        assert_eq!(cfg.language, "auto");
        assert_eq!(cfg.commit, CommitMode::Direct);
        assert!(cfg.plugin_id.is_empty());
    }

    #[test]
    fn unknown_language_falls_back_to_auto() {
        let cfg = SpeechConfig {
            language: "klingon".into(),
            plugin_id: "  com.example.asr  ".into(),
            ..SpeechConfig::default()
        }
        .normalized();
        assert_eq!(cfg.language, "auto");
        assert_eq!(cfg.plugin_id, "com.example.asr");
    }

    #[test]
    fn mode_and_commit_index_roundtrip() {
        for mode in [SpeechMode::Local, SpeechMode::Online] {
            assert_eq!(SpeechMode::from_index(mode.index()), mode);
            assert!(!mode.hint().is_empty());
        }
        for commit in [CommitMode::Direct, CommitMode::Candidate] {
            assert_eq!(CommitMode::from_index(commit.index()), commit);
            assert!(!commit.hint().is_empty());
        }
        // 越界索引不能 panic，也不能显示成不存在的模式。
        assert_eq!(SpeechMode::from_index(99), SpeechMode::Local);
        assert_eq!(CommitMode::from_index(99), CommitMode::Direct);
        assert_eq!(SpeechMode::LABELS.len(), 2);
        assert_eq!(CommitMode::LABELS.len(), 2);
    }

    #[test]
    fn language_label_covers_every_option() {
        for (code, label) in LANGUAGES {
            assert_eq!(language_label(code), label);
        }
        assert_eq!(language_label("unknown"), "自动识别");
    }

    #[test]
    fn scan_filters_by_plugin_type_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (id, ty) in [
            ("b.asr", "speech"),
            ("a.asr", "speech"),
            ("c.emoji", "emoji"),
        ] {
            let plugin_dir = root.join(id);
            std::fs::create_dir_all(&plugin_dir).unwrap();
            std::fs::write(
                plugin_dir.join("manifest.json"),
                format!(
                    "{{\"id\":\"{id}\",\"name\":\"{id}\",\"type\":\"{ty}\",\"version\":\"1.0.0\"}}"
                ),
            )
            .unwrap();
        }
        let plugins = scan_speech_plugins_in(root);
        let ids: Vec<&str> = plugins.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["a.asr", "b.asr"], "只收 speech 类型，且按目录名排序");
        // 没有注册记录时按启用处理（与插件管理页一致）。
        assert!(plugins.iter().all(|p| p.enabled));
    }
}
