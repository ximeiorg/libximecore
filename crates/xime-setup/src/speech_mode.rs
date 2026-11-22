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

// 模式 / 上屏方式 / 语种 / `speech.toml` 的读写全部在 libximecore 的
// `xime_config::speech_config`：设置程序与各平台输入法必须读同一份实现，
// 否则「设置里关了、输入法还照录」（同剪贴板库 D21 的教训）。这里只做转出，
// 让设置页继续用 `crate::speech_mode::*` 这套老路径。
pub use xime_config::speech_config::{
    config_path, language_label, read_config, write_config, CommitMode, SpeechConfig, SpeechMode,
    LANGUAGES,
};

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
    /// 语音输入总开关（写进 `speech.toml` 的 `enabled`，输入法按 mtime 热加载）。
    pub enabled: bool,
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
            enabled: cfg.enabled,
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
        // 总开关关着时页面只显示开关本身，不必白读一次插件 schema。
        if state.enabled && state.mode == SpeechMode::Online && state.selected_plugin().is_some() {
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

    /// 语音输入总开关：一键关闭后，快捷键、候选栏 🎙️、菜单栏「语音输入」
    /// 一起失效（输入法读同一份 `speech.toml`），本页也不再拉起语音服务。
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled == self.enabled {
            return;
        }
        self.enabled = enabled;
        // 读-改-写只动 `enabled` 一个键：模式 / 插件 / 语种不能因为关一次开关丢掉。
        let mut cfg = read_config();
        cfg.enabled = enabled;
        write_config(&cfg);
        self.set_message(if enabled {
            "语音输入已开启：按住 Fn / Option 即可说话"
        } else {
            "语音输入已关闭：快捷键与菜单入口同时失效"
        });
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
            ..SpeechConfig::default()
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
