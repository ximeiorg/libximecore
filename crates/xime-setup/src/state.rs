use crate::theme::{SystemTheme, ThemeColors};
use serde::Deserialize;
use sha2::Digest;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Mutex, OnceLock};
use xime_config::{
    deploy_all, get_data_dirs, ColorSchemeConfig, DarkMode, SchemaConfig, SchemaConfigManager,
    SchemaInfo, SchemaManager, XimeConfig,
};

static MARKET_TASK_RESULT: OnceLock<Mutex<Option<MarketTaskResult>>> = OnceLock::new();
static MARKET_YAML_RESULT: OnceLock<Mutex<Option<Result<String, String>>>> = OnceLock::new();
static DEPLOY_RESULT: OnceLock<Mutex<Option<Result<(), String>>>> = OnceLock::new();
static MODEL_TASK_RESULT: OnceLock<Mutex<Option<ModelTaskResult>>> = OnceLock::new();
static MODEL_YAML_RESULT: OnceLock<Mutex<Option<Result<String, String>>>> = OnceLock::new();
static PLUGIN_TASK_RESULT: OnceLock<Mutex<Option<PluginTaskResult>>> = OnceLock::new();
static PLUGIN_YAML_RESULT: OnceLock<Mutex<Option<Result<String, String>>>> = OnceLock::new();
static DOWNLOAD_PROGRESS: OnceLock<Mutex<Option<(String, f32)>>> = OnceLock::new();

fn market_task_result() -> &'static Mutex<Option<MarketTaskResult>> {
    MARKET_TASK_RESULT.get_or_init(|| Mutex::new(None))
}

fn market_yaml_result() -> &'static Mutex<Option<Result<String, String>>> {
    MARKET_YAML_RESULT.get_or_init(|| Mutex::new(None))
}

fn deploy_result() -> &'static Mutex<Option<Result<(), String>>> {
    DEPLOY_RESULT.get_or_init(|| Mutex::new(None))
}

fn model_task_result() -> &'static Mutex<Option<ModelTaskResult>> {
    MODEL_TASK_RESULT.get_or_init(|| Mutex::new(None))
}

fn model_yaml_result() -> &'static Mutex<Option<Result<String, String>>> {
    MODEL_YAML_RESULT.get_or_init(|| Mutex::new(None))
}

fn plugin_task_result() -> &'static Mutex<Option<PluginTaskResult>> {
    PLUGIN_TASK_RESULT.get_or_init(|| Mutex::new(None))
}

fn plugin_yaml_result() -> &'static Mutex<Option<Result<String, String>>> {
    PLUGIN_YAML_RESULT.get_or_init(|| Mutex::new(None))
}

/// 下载进度（任务 id, 0.0~1.0），由下载线程更新、UI 轮询取走。
fn download_progress() -> &'static Mutex<Option<(String, f32)>> {
    DOWNLOAD_PROGRESS.get_or_init(|| Mutex::new(None))
}

enum MarketTaskResult {
    DownloadDone(String),
    InstallDone(String),
    UninstallDone(String),
    DeleteDone(String),
    Error(String),
}

enum ModelTaskResult {
    DownloadDone(String),
    DeleteDone(String),
    Error(String),
}

enum PluginTaskResult {
    InstallDone(String),
    UninstallDone(String),
    ToggleDone(String, bool),
    Error(String),
}

static NOTIFY_DEPLOY: OnceLock<fn()> = OnceLock::new();
static NOTIFY_RELOAD_STYLE: OnceLock<fn()> = OnceLock::new();
static NOTIFY_SELECT_SCHEMA: OnceLock<fn(&str) -> bool> = OnceLock::new();
static NOTIFY_MESSAGE: OnceLock<fn(&str, &str)> = OnceLock::new();
static NOTIFY_RELOAD_PLUGINS: OnceLock<fn()> = OnceLock::new();

/// 设置宿主进程的「部署后重载」回调（daemon 重载配置）。
pub fn set_notify_deploy(f: fn()) {
    let _ = NOTIFY_DEPLOY.set(f);
}

/// 设置宿主进程的「样式重载」回调（daemon 重新加载配色/字号）。
pub fn set_notify_reload_style(f: fn()) {
    let _ = NOTIFY_RELOAD_STYLE.set(f);
}

/// 设置宿主进程的「切换当前输入方案」回调（通过 IPC 发送 SelectSchema 命令）。
/// 返回是否发送成功（服务器是否运行）。
pub fn set_notify_select_schema(f: fn(&str) -> bool) {
    let _ = NOTIFY_SELECT_SCHEMA.set(f);
}

/// 设置宿主进程的「结果消息」回调（部署/保存等成功失败提示）。
/// 宿主可用它发系统通知；未注册则消息仅在页面底部显示。
pub fn set_notify_message(f: fn(&str, &str)) {
    let _ = NOTIFY_MESSAGE.set(f);
}

/// 设置宿主进程的「插件变更」回调（daemon 重载插件：安装/卸载/启停后触发）。
pub fn set_notify_reload_plugins(f: fn()) {
    let _ = NOTIFY_RELOAD_PLUGINS.set(f);
}

fn notify_daemon_reload_plugins() {
    if let Some(f) = NOTIFY_RELOAD_PLUGINS.get() {
        f();
    }
}

fn notify_daemon_reload() -> bool {
    if let Some(f) = NOTIFY_DEPLOY.get() {
        f();
        true
    } else {
        false
    }
}

fn notify_daemon_reload_style() {
    if let Some(f) = NOTIFY_RELOAD_STYLE.get() {
        f();
    }
}

fn notify_select_schema(schema_id: &str) -> bool {
    if let Some(f) = NOTIFY_SELECT_SCHEMA.get() {
        f(schema_id)
    } else {
        false
    }
}

/// 方案市场下载目录：~/.config/xime/markets/（与 Xime 的 market 约定对齐）。
fn markets_dir() -> std::path::PathBuf {
    let (_, user_data_dir) = get_data_dirs();
    user_data_dir
        .parent()
        .map(|p| p.join("markets"))
        .unwrap_or_else(|| {
            let base = std::env::var("LOCALAPPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::env::temp_dir());
            base.join(xime_config::app_metadata().config_dir_name)
                .join("markets")
        })
}

/// 设置应用的 UI 消息（iced）。
#[derive(Debug, Clone)]
pub enum Message {
    /// 切换左侧导航页面。
    PageSelected(usize),
    /// 输入方案页：切换「已安装 / 已下载」标签。
    SchemaTab(usize),
    /// 输入方案页：选择某个已安装方案。
    SelectSchema(usize),
    /// 部署方案（输入方案页 / 快捷键页 / 方案市场）。
    DeploySchemas,
    /// 安装方案（已下载包 / 方案市场）。
    InstallSchema(String),
    /// 卸载方案。
    UninstallSchema(String),
    /// 方案市场：下载方案。
    DownloadSchema(String),
    /// 扩展商店：下载模型。
    DownloadModel(String),
    /// 扩展商店：删除本地已下载的模型。
    DeleteModel(String),
    /// 扩展商店：下载插件包。
    DownloadPlugin(String),
    /// 扩展商店：安装已下载的插件包。
    InstallPlugin(String),
    /// 扩展商店：卸载插件。
    UninstallPlugin(String),
    /// 插件管理：确认卸载（二次确认状态）。
    ConfirmUninstallPlugin(String),
    /// 插件管理：刷新已安装插件列表。
    RefreshPlugins,
    /// 插件管理：取消卸载确认。
    CancelUninstallPlugin,
    /// 扩展商店：启用 / 禁用插件。
    TogglePlugin(String, bool),
    /// 扩展商店：方案市场 / 模型市场加载失败后重试。
    MarketRetry,
    /// 输入方案页：打开用户数据目录。
    OpenUserDataDir,
    /// 扩展商店：切换「方案 / 模型」Tab。
    StoreTab(usize),
    /// 扩展商店：分类筛选（"" 表示全部）。
    StoreTagSelected(String),
    /// 扩展商店：选择方案版本。
    SchemaVersionSelected(String, String),
    /// 扩展商店：选择模型版本。
    ModelVersionSelected(String, String),
    /// 外观：字号变更。
    FontSizeChanged(f64),
    /// 外观：候选词数量变更。
    CandidateCountChanged(i32),
    /// 外观：圆角大小变更。
    CornerRadiusChanged(f64),
    /// 外观：浅色模式配色方案变更。
    ColorSchemeLightChanged(String),
    /// 外观：深色模式配色方案变更。
    ColorSchemeDarkChanged(String),
    /// 外观：深色模式变更（0=浅色, 1=深色, 2=跟随系统）。
    DarkModeChanged(u8),
    /// 外观：保存。
    SaveAppearance,
    #[cfg(feature = "smart-suggestion-page")]
    SaveSmartSuggestion,
    #[cfg(feature = "clipboard-page")]
    ClearClipboardHistory,
    #[cfg(feature = "clipboard-page")]
    ServerStart,
    #[cfg(feature = "clipboard-page")]
    ServerStop,
    #[cfg(feature = "clipboard-page")]
    ServerRestart,
    #[cfg(feature = "clipboard-page")]
    ServerAddrChanged(String),
    #[cfg(feature = "clipboard-page")]
    ServerUsernameChanged(String),
    #[cfg(feature = "clipboard-page")]
    ServerPasswordChanged(String),
    #[cfg(feature = "clipboard-page")]
    OpenSyncDataDir,
    #[cfg(feature = "pair-page")]
    StartPairing,
    /// 订阅轮询：后台任务结果。
    BackgroundPoll,
}
#[derive(Clone)]
pub struct SettingsState {
    pub appearance: AppearanceState,
    pub input_schema: InputSchemaState,
    pub system_theme: SystemTheme,
    pub schemas_loaded: bool,
    pub market_schema: MarketSchemaState,
    pub market_model: MarketModelState,
    pub market_plugin: MarketPluginState,
    /// 插件管理：等待确认卸载的插件 id（None=无）。
    pub plugin_uninstall_confirm: Option<String>,
    pub current_page: usize,
    #[cfg(feature = "smart-suggestion-page")]
    pub smart_suggestion: SmartSuggestionState,
    #[cfg(feature = "pair-page")]
    pub pair: PairState,
    #[cfg(feature = "clipboard-page")]
    pub clipboard: ClipboardState,
    #[cfg(target_os = "linux")]
    pub sync: SyncState,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsState {
    pub fn new() -> Self {
        let mut state = Self {
            appearance: AppearanceState::default(),
            input_schema: InputSchemaState::default(),
            market_schema: MarketSchemaState::default(),
            market_model: MarketModelState::default(),
            market_plugin: MarketPluginState::default(),
            plugin_uninstall_confirm: None,
            system_theme: SystemTheme::detect(),
            schemas_loaded: false,
            current_page: 0,
            #[cfg(feature = "smart-suggestion-page")]
            smart_suggestion: SmartSuggestionState::default(),
            #[cfg(feature = "pair-page")]
            pair: PairState::default(),
            #[cfg(feature = "clipboard-page")]
            clipboard: ClipboardState::default(),
            #[cfg(target_os = "linux")]
            sync: SyncState::default(),
        };
        state.load_color_schemes();
        state.load_schemas();
        state.load_schema_config();
        state.refresh_installed_plugins();
        state.start_load_market();
        state.start_load_models();
        state.start_load_plugins();
        state
    }

    pub fn colors(&self) -> ThemeColors {
        let primary_color = self.get_primary_color();
        ThemeColors::from_theme(&self.system_theme, primary_color)
    }

    fn get_primary_color(&self) -> u32 {
        let is_dark = self.appearance.dark_mode.is_dark(self.system_theme.is_dark());
        let scheme_name = self.appearance.color_scheme.scheme_name(is_dark);
        self.appearance
            .available_color_schemes
            .iter()
            .find(|(id, _, _)| *id == scheme_name)
            .map(|(_, _, color)| *color)
            .unwrap_or(0x8F73E2)
    }

    pub fn load_schemas(&mut self) {
        if self.schemas_loaded {
            return;
        }
        if let Ok(manager) = SchemaManager::new() {
            let schemas = manager.get_schema_list();
            self.input_schema.available_schemas = schemas;
            self.schemas_loaded = true;
        }
    }

    pub fn load_schema_config(&mut self) {
        if self.input_schema.config_loaded {
            return;
        }
        if self.input_schema.selected_schema >= self.input_schema.available_schemas.len() {
            return;
        }
        let schema_id =
            &self.input_schema.available_schemas[self.input_schema.selected_schema].schema_id;
        if let Ok(manager) = SchemaConfigManager::new(schema_id) {
            self.input_schema.schema_config = manager.get_config();
            self.input_schema.config_loaded = true;
        }
    }

    pub fn load_color_schemes(&mut self) {
        if self.appearance.color_schemes_loaded {
            return;
        }
        let config = XimeConfig::load();
        self.appearance.color_scheme = config.style.color_scheme.clone();
        self.appearance.dark_mode = config.style.dark_mode;
        self.appearance.available_color_schemes = config
            .color_schemes
            .iter()
            .map(|(id, scheme)| (id.clone(), scheme.name.clone(), scheme.primary_color))
            .collect();
        self.appearance.font_size = config.style.font_size as f64;
        self.appearance.candidate_count = config.style.candidate_count;
        self.appearance.corner_radius = config.style.corner_radius as f64;
        self.appearance.color_schemes_loaded = true;
    }

    pub fn save_color_scheme(&self) -> Result<(), String> {
        let mut config = XimeConfig::load();
        config.style.color_scheme = self.appearance.color_scheme.clone();
        config.style.dark_mode = self.appearance.dark_mode;
        config.save()?;
        notify_daemon_reload_style();
        Ok(())
    }

    pub fn save_appearance(&self) -> Result<(), String> {
        let mut config = XimeConfig::load();
        config.style.font_size = self.appearance.font_size as f32;
        config.style.candidate_count = self.appearance.candidate_count;
        config.style.corner_radius = self.appearance.corner_radius as f32;
        config.save()?;
        notify_daemon_reload_style();
        Ok(())
    }

    pub fn save_schema(&self) -> Result<(), String> {
        if self.input_schema.selected_schema >= self.input_schema.available_schemas.len() {
            return Ok(());
        }
        let selected_id =
            &self.input_schema.available_schemas[self.input_schema.selected_schema].schema_id;

        // 优先通知运行中的宿主进程（若已注册 SelectSchema 回调）。
        if notify_select_schema(selected_id) {
            return Ok(());
        }

        // 宿主未运行/未注册：改为持久化方案列表（选中方案置顶），
        // 等效于 RimeSwitcher 的 schema_list 设置，下次启动生效。
        let manager = SchemaManager::new()?;
        let mut ids: Vec<String> = manager.get_schema_list_ids();
        if ids.is_empty() {
            ids = self
                .input_schema
                .available_schemas
                .iter()
                .map(|s| s.schema_id.clone())
                .collect();
        }
        ids.retain(|id| id != selected_id);
        ids.insert(0, selected_id.clone());
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        manager.set_schema_list(&refs)?;
        manager.save()?;

        xime_config::rime_deploy::deploy_all_schemas().map_err(|e| format!("部署失败: {}", e))
    }

    pub fn save_schema_config(&self) -> Result<(), String> {
        if self.input_schema.selected_schema >= self.input_schema.available_schemas.len() {
            return Ok(());
        }

        let schema_id =
            &self.input_schema.available_schemas[self.input_schema.selected_schema].schema_id;
        let manager = SchemaConfigManager::new(schema_id)?;

        let config = &self.input_schema.schema_config;

        if let Some(v) = config.speller.max_code_length {
            manager.set_int("speller/max_code_length", v)?;
        }
        if let Some(v) = config.speller.auto_select {
            manager.set_bool("speller/auto_select", v)?;
        }
        if let Some(v) = &config.speller.auto_clear {
            if !v.is_empty() {
                manager.set_string("speller/auto_clear", v)?;
            }
        }

        if let Some(v) = config.translator.enable_charset_filter {
            manager.set_bool("translator/enable_charset_filter", v)?;
        }
        if let Some(v) = config.translator.enable_completion {
            manager.set_bool("translator/enable_completion", v)?;
        }
        if let Some(v) = config.translator.enable_sentence {
            manager.set_bool("translator/enable_sentence", v)?;
        }
        if let Some(v) = config.translator.enable_user_dict {
            manager.set_bool("translator/enable_user_dict", v)?;
        }
        if let Some(v) = config.translator.enable_encoder {
            manager.set_bool("translator/enable_encoder", v)?;
        }
        if let Some(v) = config.translator.encode_commit_history {
            manager.set_bool("translator/encode_commit_history", v)?;
        }
        if let Some(v) = config.translator.max_phrase_length {
            manager.set_int("translator/max_phrase_length", v)?;
        }

        if let Some(v) = &config.reverse_lookup.prefix {
            manager.set_string("reverse_lookup/prefix", v)?;
        }
        if let Some(v) = &config.reverse_lookup.suffix {
            manager.set_string("reverse_lookup/suffix", v)?;
        }

        if let Some(v) = &config.tradition.opencc_config {
            manager.set_string("tradition/opencc_config", v)?;
        }

        manager.save()?;

        Ok(())
    }

    // ---- 部署（后台线程执行，结果经轮询回收） ----

    /// 显示结果消息：触发系统通知回调（若宿主注册了 `set_notify_message`）。
    pub fn show_message(&mut self, msg: String) {
        if let Some(f) = NOTIFY_MESSAGE.get() {
            f(xime_config::app_metadata().display_name, &msg);
        }
    }

    pub fn start_deploy(&mut self) {
        if deploy_result().lock().unwrap().is_some() {
            return;
        }
        self.show_message("正在部署…".to_string());
        std::thread::spawn(|| {
            let result = deploy_all().map_err(|e| e.to_string());
            *deploy_result().lock().unwrap() = Some(result);
        });
    }

    pub fn poll_deploy(&mut self) {
        let result = deploy_result().lock().unwrap().take();
        if let Some(result) = result {
            match result {
                Ok(()) => {
                    self.show_message(if notify_daemon_reload() {
                        "部署成功！配置已重载。".to_string()
                    } else {
                        "部署成功！(服务器未运行，配置将在下次启动时生效)".to_string()
                    });
                }
                Err(e) => {
                    self.show_message(format!("部署失败: {}", e));
                }
            }
        }
    }

    // ---- 扩展商店：方案市场 ----

    pub fn start_load_market(&mut self) {
        if self.market_schema.loaded || self.market_schema.loading {
            return;
        }
        self.market_schema.loading = true;

        std::thread::spawn(|| {
            let result = (|| -> Result<String, String> {
                ureq::get("https://index.ximei.me/rimes/index.yaml")
                    .call()
                    .map_err(|e| format!("网络请求失败: {}", e))?
                    .into_body()
                    .read_to_string()
                    .map_err(|e| format!("读取响应失败: {}", e))
            })();

            *market_yaml_result().lock().unwrap() = Some(result);
        });
    }

    /// 轮询方案索引结果。返回是否有变化。
    pub fn poll_market_yaml(&mut self) -> bool {
        if !self.market_schema.loading {
            return false;
        }
        let result = market_yaml_result().lock().unwrap().take();
        let Some(result) = result else {
            return false;
        };
        match result {
            Ok(text) => match serde_yaml::from_str::<SchemaIndex>(&text) {
                Ok(index) => {
                    self.market_schema.installed_ids = self.get_installed_schema_ids();
                    self.market_schema.downloaded_ids = self.get_cached_schema_ids();
                    self.market_schema.schemas = index.schemas;
                    self.market_schema.updated_at = index.updated_at;
                    self.market_schema.loaded = true;
                    self.market_schema.loading = false;
                    self.market_schema.error = None;
                }
                Err(e) => {
                    self.market_schema.loading = false;
                    self.market_schema.error = Some(format!("解析失败: {}", e));
                }
            },
            Err(e) => {
                self.market_schema.loading = false;
                self.market_schema.error = Some(e);
            }
        }
        true
    }

    /// 轮询方案市场后台任务结果。返回是否有变化。
    pub fn poll_market_task(&mut self) -> bool {
        let result = market_task_result().lock().unwrap().take();
        let Some(result) = result else {
            return false;
        };
        match result {
            MarketTaskResult::DownloadDone(id) => {
                if !self.market_schema.downloaded_ids.contains(&id) {
                    self.market_schema.downloaded_ids.push(id);
                }
            }
            MarketTaskResult::InstallDone(id) => {
                if !self.market_schema.installed_ids.contains(&id) {
                    self.market_schema.installed_ids.push(id);
                }
            }
            MarketTaskResult::UninstallDone(id) => {
                self.market_schema.installed_ids.retain(|i| i != &id);
            }
            MarketTaskResult::DeleteDone(id) => {
                self.market_schema.downloaded_ids.retain(|i| i != &id);
            }
            MarketTaskResult::Error(e) => {
                self.market_schema.install_message = Some(e);
                self.market_schema.install_message_since = Some(std::time::Instant::now());
            }
        }
        self.market_schema.downloading = None;
        self.market_schema.installing = None;
        true
    }

    /// 轮询扩展商店下载进度（方案 / 模型通用）。返回是否有变化。
    fn poll_download_progress(&mut self) -> bool {
        let result = download_progress().lock().unwrap().take();
        let Some((id, progress)) = result else {
            return false;
        };
        if self.market_schema.downloading.as_deref() == Some(&id) {
            self.market_schema.download_progress = Some(progress);
        }
        if self.market_model.downloading.as_deref() == Some(&id) {
            self.market_model.download_progress = Some(progress);
        }
        true
    }

    // ---- 扩展商店：模型市场 ----

    pub fn start_load_models(&mut self) {
        if self.market_model.loaded || self.market_model.loading {
            return;
        }
        self.market_model.loading = true;

        std::thread::spawn(|| {
            let result = (|| -> Result<String, String> {
                ureq::get("https://index.ximei.me/models/index.yaml")
                    .call()
                    .map_err(|e| format!("网络请求失败: {}", e))?
                    .into_body()
                    .read_to_string()
                    .map_err(|e| format!("读取响应失败: {}", e))
            })();

            *model_yaml_result().lock().unwrap() = Some(result);
        });
    }

    /// 轮询模型索引结果。返回是否有变化。
    pub fn poll_model_yaml(&mut self) -> bool {
        if !self.market_model.loading {
            return false;
        }
        let result = model_yaml_result().lock().unwrap().take();
        let Some(result) = result else {
            return false;
        };
        match result {
            Ok(text) => match serde_yaml::from_str::<ModelIndex>(&text) {
                Ok(index) => {
                    self.market_model.downloaded_ids = self.get_cached_model_ids();
                    self.market_model.models = index.models;
                    self.market_model.updated_at = index.updated_at;
                    self.market_model.loaded = true;
                    self.market_model.loading = false;
                    self.market_model.error = None;
                }
                Err(e) => {
                    self.market_model.loading = false;
                    self.market_model.error = Some(format!("解析失败: {}", e));
                }
            },
            Err(e) => {
                self.market_model.loading = false;
                self.market_model.error = Some(e);
            }
        }
        true
    }

    /// 轮询模型市场后台任务结果。返回是否有变化。
    pub fn poll_model_task(&mut self) -> bool {
        let result = model_task_result().lock().unwrap().take();
        let Some(result) = result else {
            return false;
        };
        match result {
            ModelTaskResult::DownloadDone(id) => {
                if !self.market_model.downloaded_ids.contains(&id) {
                    self.market_model.downloaded_ids.push(id);
                }
            }
            ModelTaskResult::DeleteDone(id) => {
                self.market_model.downloaded_ids.retain(|i| i != &id);
            }
            ModelTaskResult::Error(e) => {
                self.market_model.install_message = Some(e);
                self.market_model.install_message_since = Some(std::time::Instant::now());
            }
        }
        self.market_model.downloading = None;
        self.market_model.download_progress = None;
        true
    }

    /// 统一回收后台任务结果（由轮询订阅调用）。
    pub fn poll_background(&mut self) {
        self.poll_deploy();
        self.poll_market_yaml();
        self.poll_model_yaml();
        self.poll_plugin_yaml();
        self.poll_market_task();
        self.poll_model_task();
        self.poll_plugin_task();
        self.poll_download_progress();
        self.expire_install_messages();
        #[cfg(feature = "clipboard-page")]
        self.clipboard.poll();
    }

    /// 扩展商店安装/卸载消息 4 秒后自动消失。
    fn expire_install_messages(&mut self) {
        let now = std::time::Instant::now();
        let expired = std::time::Duration::from_secs(4);
        for (msg, since) in [
            (
                &mut self.market_schema.install_message,
                &mut self.market_schema.install_message_since,
            ),
            (
                &mut self.market_model.install_message,
                &mut self.market_model.install_message_since,
            ),
            (
                &mut self.market_plugin.install_message,
                &mut self.market_plugin.install_message_since,
            ),
        ] {
            if msg.is_some()
                && since
                    .map(|t| now.duration_since(t) > expired)
                    .unwrap_or(true)
            {
                *msg = None;
                *since = None;
            }
        }
    }

    /// 重新加载扩展商店（方案 + 模型索引）。
    pub fn refresh_store(&mut self) {
        self.market_schema.loaded = false;
        self.market_schema.loading = false;
        self.market_schema.error = None;
        self.start_load_market();
        self.market_model.loaded = false;
        self.market_model.loading = false;
        self.market_model.error = None;
        self.start_load_models();
        self.market_plugin.loaded = false;
        self.market_plugin.loading = false;
        self.market_plugin.error = None;
        self.start_load_plugins();
    }

    // ---- 扩展商店：插件市场 ----

    pub fn start_load_plugins(&mut self) {
        if self.market_plugin.loaded || self.market_plugin.loading {
            return;
        }
        self.market_plugin.loading = true;

        std::thread::spawn(|| {
            let result = (|| -> Result<String, String> {
                ureq::get("https://index.ximei.me/plugins/index.yaml")
                    .call()
                    .map_err(|e| format!("网络请求失败: {}", e))?
                    .into_body()
                    .read_to_string()
                    .map_err(|e| format!("读取响应失败: {}", e))
            })();

            *plugin_yaml_result().lock().unwrap() = Some(result);
        });
    }

    /// 轮询插件索引结果。返回是否有变化。
    pub fn poll_plugin_yaml(&mut self) -> bool {
        if !self.market_plugin.loading {
            return false;
        }
        let result = plugin_yaml_result().lock().unwrap().take();
        let Some(result) = result else {
            return false;
        };
        match result {
            Ok(text) => match serde_yaml::from_str::<PluginIndex>(&text) {
                Ok(index) => {
                    self.market_plugin.installed = self.installed_plugins();
                    self.market_plugin.plugins = index.plugins;
                    self.market_plugin.updated_at = index.updated_at;
                    self.market_plugin.loaded = true;
                    self.market_plugin.loading = false;
                    self.market_plugin.error = None;
                }
                Err(e) => {
                    self.market_plugin.loading = false;
                    self.market_plugin.error = Some(format!("解析失败: {}", e));
                }
            },
            Err(e) => {
                self.market_plugin.loading = false;
                self.market_plugin.error = Some(e);
            }
        }
        true
    }

    /// 轮询插件市场后台任务结果。返回是否有变化。
    pub fn poll_plugin_task(&mut self) -> bool {
        let result = plugin_task_result().lock().unwrap().take();
        let Some(result) = result else {
            return false;
        };
        match result {
            PluginTaskResult::InstallDone(id) => {
                self.market_plugin.installed = self.installed_plugins();
                self.market_plugin.downloaded_ids.retain(|i| i != &id);
                notify_daemon_reload_plugins();
            }
            PluginTaskResult::UninstallDone(id) => {
                self.market_plugin.installed = self.installed_plugins();
                self.market_plugin.downloaded_ids.retain(|i| i != &id);
                notify_daemon_reload_plugins();
            }
            PluginTaskResult::ToggleDone(id, enabled) => {
                if let Some(p) = self.market_plugin.installed.iter_mut().find(|p| p.id == id) {
                    p.enabled = enabled;
                }
                notify_daemon_reload_plugins();
            }
            PluginTaskResult::Error(e) => {
                self.market_plugin.install_message = Some(e);
                self.market_plugin.install_message_since = Some(std::time::Instant::now());
            }
        }
        self.market_plugin.downloading = None;
        self.market_plugin.installing = None;
        true
    }

    pub fn download_market_plugin(&mut self, plugin_id: &str) {
        if self.market_plugin.downloading.is_some() || self.market_plugin.installing.is_some() {
            return;
        }

        let plugin = match self
            .market_plugin
            .plugins
            .iter()
            .find(|p| p.id == plugin_id)
        {
            Some(p) => p.clone(),
            None => return,
        };

        self.market_plugin.downloading = Some(plugin_id.to_string());
        self.market_plugin.download_progress = None;
        self.market_plugin.install_message = None;

        // 下载即安装：下载到临时 .xipk 后立即解压注册，成功后删除临时包。
        std::thread::spawn(move || {
            let result = do_download_plugin(&plugin).and_then(|xipk| {
                let install = plugin_manager()
                    .install_from_zip(&xipk, true)
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!("{}", e));
                std::fs::remove_file(&xipk).ok();
                install
            });
            let task = match result {
                Ok(()) => PluginTaskResult::InstallDone(plugin.id.clone()),
                Err(e) => PluginTaskResult::Error(e.to_string()),
            };
            *plugin_task_result().lock().unwrap() = Some(task);
        });
    }

    pub fn install_market_plugin(&mut self, plugin_id: &str) {
        if self.market_plugin.installing.is_some() || self.market_plugin.downloading.is_some() {
            return;
        }

        self.market_plugin.installing = Some(plugin_id.to_string());
        self.market_plugin.install_message = None;

        let pid = plugin_id.to_string();
        std::thread::spawn(move || {
            let result = do_install_plugin(&pid);
            let task = match result {
                Ok(()) => PluginTaskResult::InstallDone(pid),
                Err(e) => PluginTaskResult::Error(e.to_string()),
            };
            *plugin_task_result().lock().unwrap() = Some(task);
        });
    }

    pub fn uninstall_market_plugin(&mut self, plugin_id: &str) {
        let pid = plugin_id.to_string();
        std::thread::spawn(move || {
            let result = plugin_manager().uninstall(&pid).map_err(|e| e.to_string());
            let task = match result {
                Ok(()) => PluginTaskResult::UninstallDone(pid),
                Err(e) => PluginTaskResult::Error(e),
            };
            *plugin_task_result().lock().unwrap() = Some(task);
        });
    }

    pub fn toggle_market_plugin(&mut self, plugin_id: &str, enabled: bool) {
        let pid = plugin_id.to_string();
        std::thread::spawn(move || {
            let result = plugin_manager()
                .set_enabled(&pid, enabled)
                .map_err(|e| e.to_string());
            let task = match result {
                Ok(()) => PluginTaskResult::ToggleDone(pid, enabled),
                Err(e) => PluginTaskResult::Error(e),
            };
            *plugin_task_result().lock().unwrap() = Some(task);
        });
    }

    pub fn installed_plugins(&self) -> Vec<xime_plugin::PluginRecord> {
        plugin_manager().list()
    }

    /// 插件管理页：重新扫描已安装插件列表。
    pub fn refresh_installed_plugins(&mut self) {
        self.market_plugin.installed = self.installed_plugins();
    }

    pub fn download_market_schema(&mut self, schema_id: &str) {
        if self.market_schema.downloading.is_some() || self.market_schema.installing.is_some() {
            return;
        }

        let schema = match self
            .market_schema
            .schemas
            .iter()
            .find(|s| s.id == schema_id)
        {
            Some(s) => s.clone(),
            None => return,
        };

        self.market_schema.downloading = Some(schema_id.to_string());
        self.market_schema.download_progress = None;
        self.market_schema.install_message = None;

        std::thread::spawn(move || {
            let result = do_download(&schema);
            let task = match result {
                Ok(()) => MarketTaskResult::DownloadDone(schema.id.clone()),
                Err(e) => MarketTaskResult::Error(e.to_string()),
            };
            *market_task_result().lock().unwrap() = Some(task);
        });
    }

    pub fn install_market_schema(&mut self, schema_id: &str) {
        if self.market_schema.installing.is_some() || self.market_schema.downloading.is_some() {
            return;
        }

        self.market_schema.installing = Some(schema_id.to_string());
        self.market_schema.install_message = None;

        let sid = schema_id.to_string();
        std::thread::spawn(move || {
            let result = do_install(&sid);
            let task = match result {
                Ok(()) => MarketTaskResult::InstallDone(sid),
                Err(e) => MarketTaskResult::Error(e.to_string()),
            };
            *market_task_result().lock().unwrap() = Some(task);
        });
    }

    pub fn delete_market_package(&mut self, schema_id: &str) {
        let sid = schema_id.to_string();
        std::thread::spawn(move || {
            let pkg_dir = markets_dir().join(&sid);
            let _ = std::fs::remove_dir_all(&pkg_dir);
            let task = MarketTaskResult::DeleteDone(sid);
            *market_task_result().lock().unwrap() = Some(task);
        });
    }

    pub fn uninstall_market_schema(&mut self, schema_id: &str) {
        if self.market_schema.installing.is_some() || self.market_schema.downloading.is_some() {
            return;
        }

        self.market_schema.installing = Some(schema_id.to_string());
        self.market_schema.install_message = None;

        let sid = schema_id.to_string();
        std::thread::spawn(move || {
            let result = do_uninstall(&sid);
            let task = match result {
                Ok(()) => MarketTaskResult::UninstallDone(sid),
                Err(e) => MarketTaskResult::Error(e.to_string()),
            };
            *market_task_result().lock().unwrap() = Some(task);
        });
    }

    pub fn download_market_model(&mut self, model_id: &str) {
        if self.market_model.downloading.is_some() {
            return;
        }

        let model = match self.market_model.models.iter().find(|m| m.id == model_id) {
            Some(m) => m.clone(),
            None => return,
        };

        self.market_model.downloading = Some(model_id.to_string());
        self.market_model.download_progress = None;
        self.market_model.install_message = None;

        std::thread::spawn(move || {
            let result = do_download_model(&model);
            let task = match result {
                Ok(()) => ModelTaskResult::DownloadDone(model.id.clone()),
                Err(e) => ModelTaskResult::Error(e.to_string()),
            };
            *model_task_result().lock().unwrap() = Some(task);
        });
    }

    pub fn delete_market_model(&mut self, model_id: &str) {
        if self.market_model.downloading.as_deref() == Some(model_id) {
            return;
        }
        let mid = model_id.to_string();
        std::thread::spawn(move || {
            let model_dir = models_dir().join(&mid);
            let _ = std::fs::remove_dir_all(&model_dir);
            let task = ModelTaskResult::DeleteDone(mid);
            *model_task_result().lock().unwrap() = Some(task);
        });
    }

    // ---- 私有辅助 ----

    fn get_installed_schema_ids(&self) -> Vec<String> {
        if let Ok(manager) = SchemaManager::new() {
            manager
                .get_schema_list()
                .into_iter()
                .map(|s| s.schema_id)
                .collect()
        } else {
            Vec::new()
        }
    }

    fn get_cached_schema_ids(&self) -> Vec<String> {
        scan_dir_ids(&markets_dir())
    }

    fn get_cached_model_ids(&self) -> Vec<String> {
        scan_dir_ids(&models_dir())
    }
}

/// 扫描目录下的子目录名（跳过隐藏项），作为已下载/已缓存列表。
fn scan_dir_ids(dir: &std::path::Path) -> Vec<String> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if entry.path().is_dir() {
                ids.push(name);
            }
        }
    }
    ids
}

/// 模型下载目录：~/.config/xime/models/<id>/。
fn models_dir() -> std::path::PathBuf {
    let (_, user_data_dir) = get_data_dirs();
    user_data_dir
        .parent()
        .map(|p| p.join("models"))
        .unwrap_or_else(|| {
            let base = std::env::var("LOCALAPPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::env::temp_dir());
            base.join(xime_config::app_metadata().config_dir_name)
                .join("models")
        })
}

/// 插件安装目录根：~/.config/xime/plugins/。
fn plugins_dir() -> std::path::PathBuf {
    let (_, user_data_dir) = get_data_dirs();
    user_data_dir
        .parent()
        .map(|p| p.join("plugins"))
        .unwrap_or_else(|| {
            let base = std::env::var("LOCALAPPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::env::temp_dir());
            base.join(xime_config::app_metadata().config_dir_name)
                .join("plugins")
        })
}

/// 构建插件管理器（注册表/配置都在 plugins 目录下）。
fn plugin_manager() -> xime_plugin::PluginManager {
    xime_plugin::PluginManager::new(plugins_dir())
}

#[cfg(feature = "smart-suggestion-page")]
#[derive(Clone, Default)]
pub struct SmartSuggestionState {
    pub enabled: bool,
    pub suggestion_count: i32,
    pub record_user_frequency: bool,
    pub auto_adjust_frequency: bool,
    pub learning_threshold: i32,
}

#[cfg(feature = "pair-page")]
#[derive(Clone, Default)]
pub struct PairState {}

#[cfg(feature = "clipboard-page")]
pub struct ClipboardState {
    /// 同步服务器配置文件（~/.config/xime/xime-sync.toml）。
    pub config_path: std::path::PathBuf,
    /// 监听地址（server.addr）。
    pub server_addr: String,
    /// 认证用户名（auth.username）。
    pub username: String,
    /// 认证密码（auth.password，写入配置文件，权限 0600）。
    pub password: String,
    /// 数据目录（server.data_dir）。
    pub data_dir: String,
    /// 服务器子进程是否运行。
    pub running: bool,
    /// 最近一次操作的状态消息。
    pub status_message: Option<String>,
    /// 服务器子进程句柄（仅启动后持有）。
    child: Option<std::process::Child>,
}

/// 设置程序管理的 server 配置片段（字段名与 xime-sync-server 配置对齐，
/// 其余字段由 server 用内嵌默认值补全）。
#[cfg(feature = "clipboard-page")]
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SyncConfigFile {
    #[serde(default)]
    server: SyncServerSection,
    #[serde(default)]
    auth: SyncAuthSection,
}

#[cfg(feature = "clipboard-page")]
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SyncServerSection {
    addr: Option<String>,
    data_dir: Option<String>,
}

#[cfg(feature = "clipboard-page")]
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SyncAuthSection {
    username: Option<String>,
    password: Option<String>,
}

#[cfg(feature = "clipboard-page")]
impl Default for ClipboardState {
    fn default() -> Self {
        Self::load()
    }
}

#[cfg(feature = "clipboard-page")]
impl Clone for ClipboardState {
    fn clone(&self) -> Self {
        Self {
            config_path: self.config_path.clone(),
            server_addr: self.server_addr.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            data_dir: self.data_dir.clone(),
            running: self.running,
            status_message: self.status_message.clone(),
            child: None,
        }
    }
}

#[cfg(feature = "clipboard-page")]
impl ClipboardState {
    pub fn load() -> Self {
        let config_path = sync_config_path();
        let mut st = Self {
            config_path,
            server_addr: "0.0.0.0:8443".to_string(),
            username: "xime".to_string(),
            password: String::new(),
            data_dir: sync_data_dir().to_string_lossy().into_owned(),
            running: false,
            status_message: None,
            child: None,
        };
        st.read_config();
        st
    }

    /// 从配置文件读取已有设置（缺失走默认值）。
    fn read_config(&mut self) {
        let Ok(content) = std::fs::read_to_string(&self.config_path) else {
            return;
        };
        let Ok(cfg) = toml::from_str::<SyncConfigFile>(&content) else {
            return;
        };
        if let Some(addr) = cfg.server.addr {
            self.server_addr = addr;
        }
        if let Some(dir) = cfg.server.data_dir {
            self.data_dir = dir;
        }
        if let Some(u) = cfg.auth.username {
            self.username = u;
        }
        if let Some(p) = cfg.auth.password {
            self.password = p;
        }
    }

    /// 保存配置到配置文件（0600 权限，密码明文仅本地可读）。
    fn write_config(&self) -> Result<(), String> {
        let cfg = SyncConfigFile {
            server: SyncServerSection {
                addr: Some(self.server_addr.clone()),
                data_dir: Some(self.data_dir.clone()),
            },
            auth: SyncAuthSection {
                username: Some(self.username.clone()),
                password: Some(self.password.clone()),
            },
        };
        let content = toml::to_string(&cfg).map_err(|e| e.to_string())?;
        let parent = self.config_path.parent().ok_or("配置目录无效")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        std::fs::write(&self.config_path, content).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&self.config_path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// 启动 xime-sync-server 子进程（派生独立进程，不随设置窗口关闭）。
    ///
    /// 密码为空时自动生成随机密码并持久化到配置（零配置启动，同 ximed 行为）。
    pub fn spawn_server(&mut self) -> Result<(), String> {
        if self.running {
            return Ok(());
        }
        if self.password.is_empty() {
            self.password = random_password();
            self.status_message = Some("已生成随机密码，客户端请使用设置页显示的密码".to_string());
        }
        self.write_config()?;
        let bin = std::env::var("XIME_SYNC_SERVER_BIN").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            let candidate = std::path::PathBuf::from(&home).join(".local/bin/xime-sync-server");
            if candidate.exists() {
                candidate.to_string_lossy().into_owned()
            } else {
                "xime-sync-server".to_string()
            }
        });
        let child = std::process::Command::new(&bin)
            .arg("--config")
            .arg(&self.config_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("启动同步服务器失败: {e}"))?;
        let pid = child.id();
        self.running = true;
        self.child = Some(child);
        self.status_message = Some(format!("服务器已启动 (PID {pid})"));
        Ok(())
    }

    /// 停止服务器子进程。
    pub fn stop_server(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            self.running = false;
            self.status_message = Some("服务器已停止".to_string());
        }
    }

    /// 轮询子进程状态（外部崩溃/被手动结束后更新 UI）。
    pub fn poll(&mut self) {
        if let Some(child) = &mut self.child {
            if let Ok(Some(status)) = child.try_wait() {
                self.running = false;
                self.child = None;
                self.status_message = Some(format!("服务器已退出 ({status})"));
            }
        }
    }
}

#[cfg(feature = "clipboard-page")]
fn config_base_dir() -> std::path::PathBuf {
    let (_, user_data_dir) = get_data_dirs();
    user_data_dir
        .parent()
        .unwrap_or(&user_data_dir)
        .to_path_buf()
}

#[cfg(feature = "clipboard-page")]
fn sync_config_path() -> std::path::PathBuf {
    config_base_dir().join("xime-sync.toml")
}

#[cfg(feature = "clipboard-page")]
fn sync_data_dir() -> std::path::PathBuf {
    config_base_dir().join("sync-data")
}

/// 生成随机认证密码（16 字节随机 → URL 安全 base64）。
#[cfg(feature = "clipboard-page")]
fn random_password() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("getrandom");
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Default)]
pub struct SyncState {
    pub url: String,
    pub username: String,
    pub password: String,
    pub is_syncing: bool,
    pub status: SyncStatus,
    pub status_message: Option<String>,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SyncStatus {
    #[default]
    Idle,
    Success,
    Error,
}

#[derive(Clone)]
pub struct AppearanceState {
    pub font_size: f64,
    pub candidate_count: i32,
    pub corner_radius: f64,
    pub color_scheme: ColorSchemeConfig,
    pub dark_mode: DarkMode,
    pub available_color_schemes: Vec<(String, String, u32)>,
    pub color_schemes_loaded: bool,
}

impl Default for AppearanceState {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            candidate_count: 5,
            corner_radius: 8.0,
            color_scheme: ColorSchemeConfig::default(),
            dark_mode: DarkMode::default(),
            available_color_schemes: Vec::new(),
            color_schemes_loaded: false,
        }
    }
}

#[derive(Clone, Default)]
pub struct InputSchemaState {
    pub selected_schema: usize,
    pub available_schemas: Vec<SchemaInfo>,
    pub schema_config: SchemaConfig,
    pub config_loaded: bool,
    pub current_tab: usize,
}

#[derive(Clone, Default)]
pub struct MarketSchemaState {
    pub schemas: Vec<MarketSchema>,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub installed_ids: Vec<String>,
    pub downloaded_ids: Vec<String>,
    pub downloading: Option<String>,
    pub download_progress: Option<f32>,
    pub installing: Option<String>,
    pub install_message: Option<String>,
    pub install_message_since: Option<std::time::Instant>,
    /// 扩展商店当前 Tab（0=方案, 1=模型）。
    pub store_tab: usize,
    /// 分类筛选（None=全部）。
    pub selected_tag: Option<String>,
    /// 每个方案选中的版本。
    pub selected_versions: HashMap<String, String>,
    /// 索引更新时间。
    pub updated_at: String,
}

#[derive(Clone, Default)]
pub struct MarketModelState {
    pub models: Vec<MarketModel>,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub downloaded_ids: Vec<String>,
    pub downloading: Option<String>,
    pub download_progress: Option<f32>,
    pub install_message: Option<String>,
    pub install_message_since: Option<std::time::Instant>,
    /// 分类筛选（None=全部）。
    pub selected_tag: Option<String>,
    /// 每个模型选中的版本。
    pub selected_versions: HashMap<String, String>,
    /// 索引更新时间。
    pub updated_at: String,
}

#[derive(Clone, Default)]
pub struct MarketPluginState {
    pub plugins: Vec<MarketPlugin>,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    /// 本地已安装插件（来自 xime-plugin registry）。
    pub installed: Vec<xime_plugin::PluginRecord>,
    pub downloaded_ids: Vec<String>,
    pub downloading: Option<String>,
    pub download_progress: Option<f32>,
    pub installing: Option<String>,
    pub install_message: Option<String>,
    pub install_message_since: Option<std::time::Instant>,
    /// 分类筛选（None=全部）。
    pub selected_tag: Option<String>,
    /// 索引更新时间。
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PluginIndex {
    pub index_version: u32,
    pub updated_at: String,
    #[serde(default)]
    pub plugins: Vec<MarketPlugin>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MarketPlugin {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "type", default)]
    pub plugin_type: String,
    #[serde(rename = "pluginType", default)]
    pub plugin_kind: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub homepage: Option<String>,
    #[serde(rename = "currentVersion")]
    pub current_version: Option<String>,
    #[serde(default)]
    pub versions: Vec<MarketPluginVersion>,
    #[serde(rename = "appVersion", default)]
    pub app_version: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MarketPluginVersion {
    pub version: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(rename = "downloadUrl", default)]
    pub download_url: Vec<MarketDownloadUrl>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SchemaIndex {
    pub index_version: u32,
    pub updated_at: String,
    pub schemas: Vec<MarketSchema>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MarketSchema {
    pub id: String,
    pub name: String,
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "type")]
    pub schema_type: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub homepage: Option<String>,
    pub versions: Vec<MarketSchemaVersion>,
    #[serde(rename = "currentVersion")]
    pub current_version: Option<String>,
    #[serde(default)]
    pub dependencies: Option<Vec<String>>,
    #[serde(default)]
    pub app_version: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MarketSchemaVersion {
    pub version: String,
    pub date: String,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(rename = "downloadUrl", default)]
    pub download_url: Vec<MarketDownloadUrl>,
    #[serde(default)]
    pub size: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MarketDownloadUrl {
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(rename = "sizeBytes", default)]
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelIndex {
    pub index_version: u32,
    pub updated_at: String,
    #[serde(default)]
    pub models: Vec<MarketModel>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MarketModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// prediction / handwriting / asr / other
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub size: String,
    #[serde(rename = "type")]
    #[serde(default)]
    pub model_type: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub homepage: Option<String>,
    #[serde(rename = "currentVersion")]
    pub current_version: Option<String>,
    #[serde(default)]
    pub versions: Vec<MarketModelVersion>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MarketModelVersion {
    pub version: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(default)]
    pub files: Vec<MarketModelFile>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MarketModelFile {
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(rename = "sizeBytes", default)]
    pub size_bytes: Option<u64>,
}

// ---- installation helpers ----

fn get_download_info(schema: &MarketSchema) -> Option<(&MarketDownloadUrl, String)> {
    let version = schema.current_version.as_deref().unwrap_or("latest");
    let info = schema
        .versions
        .iter()
        .find(|v| v.version == version || version == "latest")
        .or_else(|| schema.versions.first())?;
    let download = info.download_url.first()?;
    let ext = if download.url.ends_with(".zip") {
        ".zip"
    } else if download.url.ends_with(".tar.gz") {
        ".tar.gz"
    } else {
        return None;
    };
    Some((download, format!("{}{}", version, ext)))
}

fn do_uninstall(schema_id: &str) -> anyhow::Result<()> {
    let (_, user_data_dir) = get_data_dirs();
    let market_dir = markets_dir();
    let registry_path = market_dir.join(".registry.yaml");

    // Read registry to find installed files
    let files_to_remove: Vec<String> = if registry_path.exists() {
        let content = std::fs::read_to_string(&registry_path)?;
        #[derive(serde::Deserialize)]
        struct Entry {
            files: Vec<String>,
        }
        let registry: std::collections::HashMap<String, Entry> = serde_yaml::from_str(&content)?;
        registry
            .get(schema_id)
            .map(|e| e.files.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    for file in &files_to_remove {
        let path = user_data_dir.join(file);
        let _ = std::fs::remove_file(&path);
    }

    let manager = SchemaManager::new().map_err(|e| anyhow::anyhow!("{}", e))?;
    let current_list: Vec<String> = manager
        .get_schema_list()
        .into_iter()
        .map(|s| s.schema_id)
        .filter(|id| id != schema_id)
        .collect();
    if let Some(first) = current_list.first() {
        manager
            .set_schema_list(&[first.as_str()])
            .map_err(|e| anyhow::anyhow!("{}", e))?;
    }
    manager.save().map_err(|e| anyhow::anyhow!("{}", e))?;

    if registry_path.exists() {
        let content = std::fs::read_to_string(&registry_path)?;
        let mut registry: std::collections::HashMap<String, serde_yaml::Value> =
            serde_yaml::from_str(&content)?;
        registry.remove(schema_id);
        if let Ok(yaml) = serde_yaml::to_string(&registry) {
            let _ = std::fs::write(&registry_path, yaml);
        }
    }

    deploy_all().map_err(|e| anyhow::anyhow!("部署失败: {}", e))?;
    notify_daemon_reload();
    Ok(())
}

fn do_download(schema: &MarketSchema) -> anyhow::Result<()> {
    let (download, filename) =
        get_download_info(schema).ok_or_else(|| anyhow::anyhow!("无可用下载地址或不支持的格式"))?;

    let dest_dir = markets_dir().join(&schema.id);
    std::fs::create_dir_all(&dest_dir)?;

    let dest = dest_dir.join(&filename);
    download_file(
        &download.url,
        &dest,
        download.sha256.as_deref(),
        |progress| {
            *download_progress().lock().unwrap() = Some((schema.id.clone(), progress as f32));
        },
    )?;
    Ok(())
}

fn do_download_model(model: &MarketModel) -> anyhow::Result<()> {
    let version = model
        .versions
        .iter()
        .find(|v| Some(v.version.as_str()) == model.current_version.as_deref())
        .or_else(|| model.versions.first())
        .ok_or_else(|| anyhow::anyhow!("无可用版本"))?;
    anyhow::ensure!(
        !version.files.is_empty(),
        "无可用下载文件（version: {}）",
        version.version
    );

    let dest_dir = models_dir().join(&model.id);
    std::fs::create_dir_all(&dest_dir)?;

    let total_bytes: u64 = version
        .files
        .iter()
        .map(|f| f.size_bytes.unwrap_or(0))
        .sum();

    let mut accumulated: u64 = 0;
    for file in &version.files {
        anyhow::ensure!(!file.url.is_empty(), "缺少下载地址（{}）", file.name);
        let dest = dest_dir.join(&file.name);
        let file_bytes = file.size_bytes.unwrap_or(0);
        let sha256 = file.sha256.as_deref();
        download_file(&file.url, &dest, sha256, |progress| {
            let overall = accumulated as f64 / total_bytes.max(1) as f64
                + progress * file_bytes as f64 / total_bytes.max(1) as f64;
            *download_progress().lock().unwrap() = Some((model.id.clone(), overall as f32));
        })?;
        accumulated += file_bytes;
    }
    Ok(())
}

/// 下载插件 .xipk 到插件目录（plugins/<id>.xipk，与已安装插件 plugins/<id>/ 目录同根）。
/// 下载插件包到临时文件并返回路径（调用方负责安装后删除）。
fn do_download_plugin(plugin: &MarketPlugin) -> anyhow::Result<std::path::PathBuf> {
    let version = plugin
        .versions
        .iter()
        .find(|v| Some(v.version.as_str()) == plugin.current_version.as_deref())
        .or_else(|| plugin.versions.first())
        .ok_or_else(|| anyhow::anyhow!("无可用版本"))?;
    let download = version
        .download_url
        .first()
        .ok_or_else(|| anyhow::anyhow!("缺少下载地址"))?;
    anyhow::ensure!(!download.url.is_empty(), "缺少下载地址");

    let dest = std::env::temp_dir().join(format!("{}.xipk", plugin.id));
    download_file(
        &download.url,
        &dest,
        download.sha256.as_deref(),
        |progress| {
            *download_progress().lock().unwrap() = Some((plugin.id.clone(), progress as f32));
        },
    )?;
    Ok(dest)
}

/// 从插件目录安装已下载的插件包。
fn do_install_plugin(plugin_id: &str) -> anyhow::Result<()> {
    let xipk = plugins_dir().join(format!("{plugin_id}.xipk"));
    anyhow::ensure!(xipk.exists(), "未找到下载的插件包，请先下载");

    let manager = plugin_manager();
    manager
        .install_from_zip(&xipk, true)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    std::fs::remove_file(&xipk).ok();
    Ok(())
}

/// 下载单个文件到 dest，可选 sha256 校验；进度经回调上报（0.0~1.0）。
fn download_file(
    url: &str,
    dest: &std::path::Path,
    sha256: Option<&str>,
    on_progress: impl Fn(f64),
) -> anyhow::Result<()> {
    let mut response = ureq::get(url)
        .call()
        .map_err(|e| anyhow::anyhow!("网络请求失败: {}", e))?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    let mut reader = response.body_mut().as_reader();
    let mut hasher = sha256.map(|_| sha2::Sha256::new());
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| anyhow::anyhow!("创建目录失败: {}", e))?;
    }
    let mut output =
        std::fs::File::create(dest).map_err(|e| anyhow::anyhow!("创建文件失败: {}", e))?;

    let mut buf = [0u8; 8192];
    let mut done: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| anyhow::anyhow!("读取响应失败: {}", e))?;
        if n == 0 {
            break;
        }
        if let Some(h) = &mut hasher {
            h.update(&buf[..n]);
        }
        output
            .write_all(&buf[..n])
            .map_err(|e| anyhow::anyhow!("写入文件失败: {}", e))?;
        done += n as u64;
        if total > 0 {
            on_progress(done as f64 / total as f64);
        }
    }

    if let (Some(h), Some(expected)) = (hasher, sha256) {
        let actual = hex::encode(h.finalize());
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            std::fs::remove_file(dest).ok();
            anyhow::bail!("文件校验失败（sha256 不匹配），文件可能不完整");
        }
    }
    Ok(())
}

fn do_install(schema_id: &str) -> anyhow::Result<()> {
    let cache_schema_dir = markets_dir().join(schema_id);
    anyhow::ensure!(cache_schema_dir.exists(), "未找到缓存的下载文件");

    let archive = std::fs::read_dir(&cache_schema_dir)?
        .flatten()
        .find(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.ends_with(".zip") || n.ends_with(".tar.gz")
        })
        .ok_or_else(|| anyhow::anyhow!("未找到缓存的压缩包"))?;

    let archive_path = archive.path();
    let temp_dir = std::env::temp_dir().join(format!("xime_extract_{}", schema_id));
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).ok();
    }
    std::fs::create_dir_all(&temp_dir)?;

    let filename = archive_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    if filename.ends_with(".zip") {
        extract_zip(&archive_path, &temp_dir)?;
    } else {
        extract_tar_gz(&archive_path, &temp_dir)?;
    }

    let (_, user_data_dir) = get_data_dirs();
    let schema_files = find_schema_files(&temp_dir);
    anyhow::ensure!(
        !schema_files.is_empty(),
        "未在下载包中找到 .schema.yaml 文件"
    );

    for path in &schema_files {
        let name = path.file_name().unwrap();
        std::fs::copy(path, user_data_dir.join(name))?;
    }

    let manager = SchemaManager::new().map_err(|e| anyhow::anyhow!("{}", e))?;
    if let Some(current) = manager.get_selected_schema() {
        manager
            .set_schema_list(&[current.as_str(), schema_id])
            .map_err(|e| anyhow::anyhow!("{}", e))?;
    } else {
        manager
            .set_schema_list(&[schema_id])
            .map_err(|e| anyhow::anyhow!("{}", e))?;
    }
    manager.save().map_err(|e| anyhow::anyhow!("{}", e))?;

    deploy_all().map_err(|e| anyhow::anyhow!("部署失败: {}", e))?;
    notify_daemon_reload();

    std::fs::remove_dir_all(&temp_dir).ok();
    Ok(())
}

fn extract_zip(archive_path: &std::path::Path, dest: &std::path::Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(path) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            continue;
        };
        let target = dest.join(&path);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut output = std::fs::File::create(&target)?;
            std::io::copy(&mut entry, &mut output)?;
        }
    }
    Ok(())
}

fn extract_tar_gz(archive_path: &std::path::Path, dest: &std::path::Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let target = dest.join(&path);
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            entry.unpack(&target)?;
        }
    }
    Ok(())
}

fn find_schema_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                results.extend(find_schema_files(&path));
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".schema.yaml") {
                    results.push(path);
                }
            }
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL_INDEX_YAML: &str = r#"
index_version: 1
updated_at: '2026-08-12'
models:
- id: predictive-text-small
  name: 智能联想模型 small 版本
  author: kingzcheung
  description: 基于 ONNX 的 AI 联想词预测模型
  category: prediction
  size: 18.9 MB
  type: remote
  tags: [联想, AI, ONNX]
  homepage: https://github.com/ximeiorg/predictive-text
  currentVersion: v1.0
  versions:
  - version: v1.0
    date: '2026-08-01'
    changelog: 初始版本
    files:
    - name: vocab.json
      url: https://example.com/vocab.json
      sha256: 3f7a6aa773afe6dacf75701f7861257d36a46a26ac70c0f8ee6dd4032cc3b9c2
      size: 137.8 KB
      sizeBytes: 141139
    - name: model_int8_dynamic.onnx
      url: https://example.com/model.onnx
      sha256: e15009c84d9702056ba8b5f6c04b27ae7d0400167647a5e94cb699f24f885a9d
      size: 34.7 MB
      sizeBytes: 36353598
- id: ochwpro
  name: 手写模型
  category: handwriting
  size: 6.7 MB
  currentVersion: v1.0
  versions:
  - version: v1.0
    files:
    - name: ochwpro.onnx
      url: https://example.com/ochwpro.onnx
"#;

    #[test]
    fn parse_model_index() {
        let index: ModelIndex = serde_yaml::from_str(MODEL_INDEX_YAML).unwrap();
        assert_eq!(index.updated_at, "2026-08-12");
        assert_eq!(index.models.len(), 2);

        let model = &index.models[0];
        assert_eq!(model.id, "predictive-text-small");
        assert_eq!(model.category, "prediction");
        assert_eq!(model.current_version.as_deref(), Some("v1.0"));
        assert_eq!(model.versions[0].files.len(), 2);
        let file = &model.versions[0].files[0];
        assert_eq!(file.name, "vocab.json");
        assert_eq!(file.size_bytes, Some(141139));
        assert_eq!(
            file.sha256.as_deref(),
            Some("3f7a6aa773afe6dacf75701f7861257d36a46a26ac70c0f8ee6dd4032cc3b9c2")
        );

        let handwriting = &index.models[1];
        assert_eq!(handwriting.category, "handwriting");
        assert_eq!(handwriting.author, "");
        assert!(handwriting.versions[0].files[0].sha256.is_none());
    }

    const SCHEMA_INDEX_YAML: &str = r#"
index_version: 1
updated_at: '2026-08-12'
schemas:
- id: rime-frost
  name: 白霜拼音
  author: gaboolic
  description: 白霜拼音
  type: remote
  tags: [拼音, 双拼]
  dependencies: [luna_pinyin]
  currentVersion: 1.0.4
  versions:
  - version: 1.0.4
    date: '2026-07-10'
    downloadUrl:
    - url: https://github.com/gaboolic/rime-frost/releases/download/1.0.4/rime-frost-schemas.zip
      sha256: 4f4998ae83f63d757c0a4ace192f69d48265bddfabe231642b73e3739ed0f2f5
      size: 42 MB
"#;

    #[test]
    fn parse_schema_index_and_pick_download() {
        let index: SchemaIndex = serde_yaml::from_str(SCHEMA_INDEX_YAML).unwrap();
        assert_eq!(index.schemas.len(), 1);

        let schema = &index.schemas[0];
        assert_eq!(schema.schema_type, "remote");
        assert_eq!(schema.tags, vec!["拼音".to_string(), "双拼".to_string()]);
        assert_eq!(schema.current_version.as_deref(), Some("1.0.4"));

        let (download, filename) = get_download_info(schema).unwrap();
        assert_eq!(
            download.url,
            "https://github.com/gaboolic/rime-frost/releases/download/1.0.4/rime-frost-schemas.zip"
        );
        assert_eq!(
            download.sha256.as_deref(),
            Some("4f4998ae83f63d757c0a4ace192f69d48265bddfabe231642b73e3739ed0f2f5")
        );
        assert_eq!(filename, "1.0.4.zip");
    }

    #[test]
    fn scan_dir_ids_skips_hidden_and_files() {
        let dir = std::env::temp_dir().join(format!("xime_scan_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("model-a")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::create_dir_all(dir.join("model-b")).unwrap();
        std::fs::write(dir.join("plain-file"), b"x").unwrap();

        let mut ids = scan_dir_ids(&dir);
        ids.sort();
        assert_eq!(ids, vec!["model-a".to_string(), "model-b".to_string()]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn format_size_helper() {
        assert_eq!(crate::pages::store::format_size("42 MB"), "42 mb");
        assert_eq!(crate::pages::store::format_size("7001270"), "6.7 MB");
        assert_eq!(crate::pages::store::format_size("1024"), "1024 B");
        assert_eq!(crate::pages::store::format_size("2048"), "2.0 KB");
        assert_eq!(crate::pages::store::format_size("500"), "500 B");
    }
}
