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
static NOTIFY_SYNC_USER_DATA: OnceLock<fn() -> bool> = OnceLock::new();

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

/// 设置宿主进程的「用户资料同步」回调（IPC SyncUserData，阻塞等待完成）。
pub fn set_notify_sync_user_data(f: fn() -> bool) {
    let _ = NOTIFY_SYNC_USER_DATA.set(f);
}

/// 触发 rime 用户资料同步；false = 服务器未运行或同步失败。
#[cfg(feature = "backup-page")]
pub fn notify_sync_user_data() -> bool {
    NOTIFY_SYNC_USER_DATA.get().map(|f| f()).unwrap_or(false)
}

// ---- 词典管理回调（host 注册，Windows；对齐 weasel DictManagementDialog）----

/// 用户词典列表结果（词典名 + 快照目录）。
#[cfg(windows)]
#[derive(Clone, Debug, Default)]
pub struct DictListResult {
    pub dicts: Vec<String>,
    pub sync_dir: String,
}

#[cfg(windows)]
static NOTIFY_DICT_LIST: OnceLock<fn() -> Option<DictListResult>> = OnceLock::new();
#[cfg(windows)]
static NOTIFY_DICT_BACKUP: OnceLock<fn(&str) -> bool> = OnceLock::new();
#[cfg(windows)]
static NOTIFY_DICT_RESTORE: OnceLock<fn(&str) -> bool> = OnceLock::new();
#[cfg(windows)]
static NOTIFY_DICT_EXPORT: OnceLock<fn(&str, &str) -> Option<i32>> = OnceLock::new();
#[cfg(windows)]
static NOTIFY_DICT_IMPORT: OnceLock<fn(&str, &str) -> Option<i32>> = OnceLock::new();

/// 设置宿主进程的「列出用户词典」回调（IPC ListUserDicts）。
#[cfg(windows)]
pub fn set_notify_dict_list(f: fn() -> Option<DictListResult>) {
    let _ = NOTIFY_DICT_LIST.set(f);
}

/// 设置宿主进程的「备份用户词典」回调（IPC BackupUserDict）。
#[cfg(windows)]
pub fn set_notify_dict_backup(f: fn(&str) -> bool) {
    let _ = NOTIFY_DICT_BACKUP.set(f);
}

/// 设置宿主进程的「恢复用户词典」回调（IPC RestoreUserDict）。
#[cfg(windows)]
pub fn set_notify_dict_restore(f: fn(&str) -> bool) {
    let _ = NOTIFY_DICT_RESTORE.set(f);
}

/// 设置宿主进程的「导出用户词典」回调（IPC ExportUserDict，返回条数）。
#[cfg(windows)]
pub fn set_notify_dict_export(f: fn(&str, &str) -> Option<i32>) {
    let _ = NOTIFY_DICT_EXPORT.set(f);
}

/// 设置宿主进程的「导入用户词典」回调（IPC ImportUserDict，返回条数）。
#[cfg(windows)]
pub fn set_notify_dict_import(f: fn(&str, &str) -> Option<i32>) {
    let _ = NOTIFY_DICT_IMPORT.set(f);
}

#[cfg(windows)]
fn notify_dict_list() -> Option<DictListResult> {
    NOTIFY_DICT_LIST.get().and_then(|f| f())
}

#[cfg(windows)]
fn notify_dict_backup(dict: &str) -> bool {
    NOTIFY_DICT_BACKUP.get().map(|f| f(dict)).unwrap_or(false)
}

#[cfg(windows)]
fn notify_dict_restore(path: &str) -> bool {
    NOTIFY_DICT_RESTORE
        .get()
        .map(|f| f(path))
        .unwrap_or(false)
}

#[cfg(windows)]
fn notify_dict_export(dict: &str, path: &str) -> Option<i32> {
    NOTIFY_DICT_EXPORT.get().and_then(|f| f(dict, path))
}

#[cfg(windows)]
fn notify_dict_import(dict: &str, path: &str) -> Option<i32> {
    NOTIFY_DICT_IMPORT.get().and_then(|f| f(dict, path))
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
    /// 剪贴板历史：刷新（重读 server 持久化文件）。
    #[cfg(feature = "clipboard-page")]
    ClipboardHistoryRefresh,
    /// 剪贴板页：切换 Tab（0=历史 1=快捷发送 2=同步）。
    #[cfg(feature = "clipboard-page")]
    ClipboardTab(usize),
    /// 剪贴板历史：点击卡片选中/取消选中。
    #[cfg(feature = "clipboard-page")]
    ClipboardHistorySelected(i64),
    /// 剪贴板历史：删除单条。
    #[cfg(feature = "clipboard-page")]
    ClipboardHistoryRemove(i64),
    /// 剪贴板历史：将条目添加为快捷发送。
    #[cfg(feature = "clipboard-page")]
    QuickSendFromHistory(i64),
    /// 快捷发送：草稿触发编码变更。
    #[cfg(feature = "clipboard-page")]
    QuickSendCodeChanged(String),
    /// 快捷发送：草稿内容变更。
    #[cfg(feature = "clipboard-page")]
    QuickSendContentChanged(String),
    /// 快捷发送：打开新增弹窗。
    #[cfg(feature = "clipboard-page")]
    QuickSendOpen,
    /// 快捷发送：取消新增弹窗。
    #[cfg(feature = "clipboard-page")]
    QuickSendCancel,
    /// 快捷发送：确认添加（弹窗内）。
    #[cfg(feature = "clipboard-page")]
    QuickSendAdd,
    /// 快捷发送：点击卡片选中/取消选中。
    #[cfg(feature = "clipboard-page")]
    QuickSendSelected(i64),
    /// 快捷发送：删除条目（SQLite id）。
    #[cfg(feature = "clipboard-page")]
    QuickSendRemove(i64),
    /// 剪贴板历史：翻页（上一页 / 下一页）。
    #[cfg(feature = "clipboard-page")]
    ClipboardHistoryPrevPage,
    #[cfg(feature = "clipboard-page")]
    ClipboardHistoryNextPage,
    /// 快捷发送：翻页（上一页 / 下一页）。
    #[cfg(feature = "clipboard-page")]
    QuickSendPrevPage,
    #[cfg(feature = "clipboard-page")]
    QuickSendNextPage,
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
    /// 剪贴板同步插件：启用开关。
    #[cfg(feature = "clipboard-page")]
    SyncPluginEnabled(bool),
    /// 剪贴板同步插件：选择插件。
    #[cfg(feature = "clipboard-page")]
    SyncPluginSelected(usize),
    /// 剪贴板同步插件：编辑配置字段。
    #[cfg(feature = "clipboard-page")]
    SyncPluginFieldChanged(String, String),
    /// 剪贴板同步插件：测试连接。
    #[cfg(feature = "clipboard-page")]
    SyncPluginTest,
    #[cfg(feature = "backup-page")]
    BackupUrlChanged(String),
    #[cfg(feature = "backup-page")]
    BackupUsernameChanged(String),
    #[cfg(feature = "backup-page")]
    BackupPasswordChanged(String),
    #[cfg(feature = "backup-page")]
    BackupDirChanged(String),
    /// 云备份：切换提供者（内置 WebDAV / backup 插件）。
    #[cfg(feature = "backup-page")]
    BackupProviderChanged(usize),
    /// 云备份：编辑插件配置字段（key, value）。
    #[cfg(feature = "backup-page")]
    BackupFieldChanged(String, String),
    /// 云备份：切换备份模式（0=仅配置, 1=全量）。
    #[cfg(feature = "backup-page")]
    BackupModeChanged(u8),
    /// 云备份：测试 WebDAV 连接。
    #[cfg(feature = "backup-page")]
    BackupTest,
    /// 云备份：立即备份。
    #[cfg(feature = "backup-page")]
    BackupNow,
    /// 云备份：查看远端备份列表。
    #[cfg(feature = "backup-page")]
    BackupList,
    /// 云备份：恢复指定备份（远端路径）。
    #[cfg(feature = "backup-page")]
    BackupRestore(String),
    /// 云备份：删除指定备份（远端路径）。
    #[cfg(feature = "backup-page")]
    BackupDelete(String),
    /// 用户资料同步：立即同步（rime sync_user_data，词典快照导出+合并）。
    #[cfg(feature = "backup-page")]
    RimeSyncNow,
    /// 语音转文本：开始/停止听写。
    #[cfg(feature = "voice-page")]
    #[cfg(windows)]
    SpeechToggle,
    /// 语音转文本：清空识别文本。
    #[cfg(feature = "voice-page")]
    #[cfg(windows)]
    SpeechClear,
    /// 语音转文本：复制识别文本到剪贴板。
    #[cfg(feature = "voice-page")]
    #[cfg(windows)]
    SpeechCopy,
    /// 词典管理：刷新用户词典列表。
    #[cfg(windows)]
    DictRefresh,
    /// 词典管理：备份词典快照（词典名）。
    #[cfg(windows)]
    DictBackup(String),
    /// 词典管理：从快照文件恢复（弹文件对话框）。
    #[cfg(windows)]
    DictRestore,
    /// 词典管理：导出词典为文本（词典名，弹保存对话框）。
    #[cfg(windows)]
    DictExport(String),
    /// 词典管理：从文本导入词典（词典名，弹文件对话框）。
    #[cfg(windows)]
    DictImport(String),
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
    #[cfg(feature = "clipboard-page")]
    pub sync_plugin: SyncPluginUiState,
    /// 剪贴板历史（clipboard_history.json，server 持久化、设置页展示/清空）。
    #[cfg(feature = "clipboard-page")]
    pub clip_history: ClipboardHistoryState,
    /// 快捷发送（quick_send.yaml，设置页编辑，输入法面板后续消费同一文件）。
    #[cfg(feature = "clipboard-page")]
    pub quick_send: QuickSendState,
    /// 剪贴板页当前 Tab（0=历史 1=快捷发送 2=同步）。
    #[cfg(feature = "clipboard-page")]
    pub clipboard_tab: usize,
    #[cfg(feature = "backup-page")]
    pub backup: BackupState,
    /// rime 用户资料同步（快照目录概况 + 立即同步入口）。
    #[cfg(feature = "backup-page")]
    pub rime_sync: RimeSyncState,
    /// 语音转文本（Windows WinRT 听写；worker 归线程所有，此处只存句柄与镜像）。
    #[cfg(feature = "voice-page")]
    #[cfg(windows)]
    pub speech: SpeechState,
    /// 词典管理（用户词典列表 + 备份/恢复/导出/导入）。
    #[cfg(windows)]
    pub dict_manage: DictManageState,
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
            #[cfg(feature = "clipboard-page")]
            sync_plugin: SyncPluginUiState::default(),
            #[cfg(feature = "clipboard-page")]
            clip_history: ClipboardHistoryState::load(),
            #[cfg(feature = "clipboard-page")]
            quick_send: QuickSendState::load(),
            #[cfg(feature = "clipboard-page")]
            clipboard_tab: 0,
            #[cfg(feature = "backup-page")]
            backup: BackupState::default(),
            #[cfg(feature = "backup-page")]
            rime_sync: RimeSyncState::load(),
            #[cfg(all(feature = "voice-page", windows))]
            speech: SpeechState::default(),
            #[cfg(windows)]
            dict_manage: DictManageState::default(),
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
        let is_dark = self
            .appearance
            .dark_mode
            .is_dark(self.system_theme.is_dark());
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
                let response = http_client()
                    .get("https://index.ximei.me/rimes/index.yaml")
                    .send()
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| format!("网络请求失败: {}", e))?;
                response.text().map_err(|e| format!("读取响应失败: {}", e))
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
                let response = http_client()
                    .get("https://index.ximei.me/models/index.yaml")
                    .send()
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| format!("网络请求失败: {}", e))?;
                response.text().map_err(|e| format!("读取响应失败: {}", e))
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
        #[cfg(feature = "clipboard-page")]
        self.sync_plugin.poll();
        #[cfg(feature = "backup-page")]
        self.backup.poll();
        #[cfg(windows)]
        self.dict_manage.poll();
        #[cfg(all(feature = "voice-page", windows))]
        self.speech.poll();
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
                // v2 子索引：JS/QuickJS 插件（minHostVersion 3.0.0，对齐
                // xime-plugin quickjs 运行时）；根路径 v1 为 Lua 时代遗留索引。
                let response = http_client()
                    .get("https://index.ximei.me/plugins/v2/index.yaml")
                    .send()
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| format!("网络请求失败: {}", e))?;
                response.text().map_err(|e| format!("读取响应失败: {}", e))
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
            // 优先级：环境变量 > 设置程序同级目录（打包进 bundle）> ~/.local/bin > PATH
            let mut sibling = None;
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    let candidate = dir.join("xime-sync-server");
                    if candidate.exists() {
                        sibling = Some(candidate.to_string_lossy().into_owned());
                    }
                }
            }
            sibling.unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_default();
                let candidate = std::path::PathBuf::from(&home).join(".local/bin/xime-sync-server");
                if candidate.exists() {
                    candidate.to_string_lossy().into_owned()
                } else {
                    "xime-sync-server".to_string()
                }
            })
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

/// 共享 blocking 客户端（reqwest blocking 每个实例内建 runtime 线程，必须复用）。
pub(crate) fn http_client() -> &'static reqwest::blocking::Client {
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::blocking::Client::new)
}

#[cfg(any(feature = "clipboard-page", feature = "backup-page"))]
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

// ---- 剪贴板同步插件（clipboard-page + 与 IME 共享配置） ----------------------

/// 剪贴板同步插件发现结果。
#[cfg(feature = "clipboard-page")]
#[derive(Clone, Debug, PartialEq)]
pub struct ClipboardSyncPluginInfo {
    pub id: String,
    pub name: String,
    pub dir: std::path::PathBuf,
    /// 注册表中的启用状态（无记录默认 true，与 daemon 扫描一致）。
    pub enabled: bool,
}

/// 插件同步配置（clipboard_sync.toml，与 IME 共享）。
#[cfg(feature = "clipboard-page")]
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct ClipboardSyncConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub plugin_id: String,
}

#[cfg(feature = "clipboard-page")]
pub fn clipboard_sync_config_path() -> std::path::PathBuf {
    config_base_dir().join("clipboard_sync.toml")
}

#[cfg(feature = "clipboard-page")]
pub fn read_clipboard_sync_config() -> ClipboardSyncConfig {
    let Ok(content) = std::fs::read_to_string(clipboard_sync_config_path()) else {
        return Default::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

#[cfg(feature = "clipboard-page")]
pub fn write_clipboard_sync_config(cfg: &ClipboardSyncConfig) {
    let path = clipboard_sync_config_path();
    if let Ok(content) = toml::to_string(cfg) {
        let _ = std::fs::write(path, content);
    }
}

/// 扫描全部已安装的 clipboard_sync 类型插件（含未启用，对齐 Android
/// `getAllInstalledPlugins` 按分类过滤：下拉选择即激活，不应把停用的藏起来）。
#[cfg(feature = "clipboard-page")]
pub fn scan_clipboard_sync_plugins() -> Vec<ClipboardSyncPluginInfo> {
    let root = config_base_dir().join("plugins");
    let manager = xime_plugin::PluginManager::new(&root);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return out;
    };
    let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let Ok(manifest) = xime_plugin::PluginManifest::from_dir(&dir) else {
            continue;
        };
        if manifest.plugin_type() != xime_plugin::PluginType::ClipboardSync {
            continue;
        }
        let enabled = manager.get(&manifest.id).map(|r| r.enabled).unwrap_or(true);
        out.push(ClipboardSyncPluginInfo {
            id: manifest.id,
            name: manifest.name,
            dir,
            enabled,
        });
    }
    out
}

// ------------------------------------------------------------------
// 剪贴板历史（SQLite：xime_config::clipboard_store，与 server 共享 clipboard.db）
// ------------------------------------------------------------------

/// 列表分页：每页条数（2 列 × 4 行卡片）。
#[cfg(feature = "clipboard-page")]
pub const CLIPBOARD_PAGE_SIZE: usize = 8;

/// 剪贴板历史页面状态：SQLite 读取展示 + 清空。
#[cfg(feature = "clipboard-page")]
#[derive(Clone)]
pub struct ClipboardHistoryState {
    /// 最新在前。
    pub items: Vec<xime_config::clipboard_store::ClipboardHistoryItem>,
    /// 当前选中的历史条目 id（点击卡片选中，再点取消）。
    pub selected: Option<i64>,
    /// 当前页（0 起）。
    pub page: usize,
}

#[cfg(feature = "clipboard-page")]
impl ClipboardHistoryState {
    pub fn load() -> Self {
        let db = xime_config::clipboard_store::default_db_path();
        xime_config::clipboard_store::migrate_legacy(
            &db,
            &db.with_file_name("clipboard_history.json"),
            &db.with_file_name("quick_send.yaml"),
        );
        Self {
            items: xime_config::clipboard_store::list_history(&db, 50).unwrap_or_default(),
            selected: None,
            page: 0,
        }
    }

    pub fn reload(&mut self) {
        self.items = xime_config::clipboard_store::list_history(
            &xime_config::clipboard_store::default_db_path(),
            50,
        )
        .unwrap_or_default();
        self.clamp_page();
    }

    /// 总页数（至少 1）。
    pub fn total_pages(&self) -> usize {
        self.items.len().div_ceil(CLIPBOARD_PAGE_SIZE).max(1)
    }

    /// 列表变化后把当前页夹回有效范围。
    pub fn clamp_page(&mut self) {
        self.page = self.page.min(self.total_pages() - 1);
    }

    pub fn prev_page(&mut self) {
        self.page = self.page.saturating_sub(1);
    }

    pub fn next_page(&mut self) {
        if self.page + 1 < self.total_pages() {
            self.page += 1;
        }
    }

    /// 点击卡片：选中 / 再点取消。
    pub fn select(&mut self, id: i64) {
        self.selected = if self.selected == Some(id) { None } else { Some(id) };
    }

    /// 删除单条历史。
    pub fn remove(&mut self, id: i64) {
        let _ = xime_config::clipboard_store::remove_history_item(
            &xime_config::clipboard_store::default_db_path(),
            id,
        );
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.reload();
    }

    /// 清空（保留快捷发送条目）。
    pub fn clear(&mut self) {
        let _ = xime_config::clipboard_store::clear_history(
            &xime_config::clipboard_store::default_db_path(),
        );
        self.items.clear();
        self.selected = None;
        self.page = 0;
    }
}

// ------------------------------------------------------------------
// 快捷发送（clipboard_entries 表 isQuickSend=1 子集，对齐 Android QuickSendItem）
// ------------------------------------------------------------------

/// 快捷发送页面状态：SQLite 列表 + 新增弹窗。
#[cfg(feature = "clipboard-page")]
#[derive(Clone)]
pub struct QuickSendState {
    pub items: Vec<xime_config::clipboard_store::QuickSendItem>,
    /// 当前选中的快捷发送条目 id（点击卡片选中，再点取消）。
    pub selected: Option<i64>,
    /// 当前页（0 起）。
    pub page: usize,
    /// 新增弹窗是否打开。
    pub dialog_open: bool,
    /// 新增弹窗草稿（内容 / 触发编码输入框）。
    pub draft_content: String,
    pub draft_code: String,
}

#[cfg(feature = "clipboard-page")]
impl QuickSendState {
    pub fn load() -> Self {
        let db = xime_config::clipboard_store::default_db_path();
        xime_config::clipboard_store::migrate_legacy(
            &db,
            &db.with_file_name("clipboard_history.json"),
            &db.with_file_name("quick_send.yaml"),
        );
        Self {
            items: xime_config::clipboard_store::list_quick_send(&db).unwrap_or_default(),
            selected: None,
            page: 0,
            dialog_open: false,
            draft_content: String::new(),
            draft_code: String::new(),
        }
    }

    pub fn reload(&mut self) {
        self.items = xime_config::clipboard_store::list_quick_send(
            &xime_config::clipboard_store::default_db_path(),
        )
        .unwrap_or_default();
        self.clamp_page();
    }

    /// 总页数（至少 1）。
    pub fn total_pages(&self) -> usize {
        self.items.len().div_ceil(CLIPBOARD_PAGE_SIZE).max(1)
    }

    /// 列表变化后把当前页夹回有效范围。
    pub fn clamp_page(&mut self) {
        self.page = self.page.min(self.total_pages() - 1);
    }

    pub fn prev_page(&mut self) {
        self.page = self.page.saturating_sub(1);
    }

    pub fn next_page(&mut self) {
        if self.page + 1 < self.total_pages() {
            self.page += 1;
        }
    }

    /// 点击卡片：选中 / 再点取消。
    pub fn select(&mut self, id: i64) {
        self.selected = if self.selected == Some(id) { None } else { Some(id) };
    }

    pub fn set_draft_code(&mut self, v: String) {
        self.draft_code = v;
    }

    pub fn set_draft_content(&mut self, v: String) {
        self.draft_content = v;
    }

    /// 打开新增弹窗（清空上次草稿）。
    pub fn open_dialog(&mut self) {
        self.dialog_open = true;
        self.draft_content.clear();
        self.draft_code.clear();
    }

    pub fn cancel_dialog(&mut self) {
        self.dialog_open = false;
        self.draft_content.clear();
        self.draft_code.clear();
    }

    /// 确认添加（内容为空时忽略并保持弹窗打开）；成功后关闭弹窗并刷新列表。
    pub fn add_draft(&mut self) -> bool {
        let content = self.draft_content.trim().to_string();
        if content.is_empty() {
            return false;
        }
        let code = self.draft_code.trim().to_string();
        let db = xime_config::clipboard_store::default_db_path();
        if xime_config::clipboard_store::add_quick_send(&db, &content, &code).is_err() {
            return false;
        }
        self.cancel_dialog();
        self.reload();
        true
    }

    pub fn remove(&mut self, id: i64) {
        let db = xime_config::clipboard_store::default_db_path();
        let _ = xime_config::clipboard_store::remove_quick_send(&db, id);
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.reload();
    }

    /// 从剪贴板历史文本直接添加为快捷发送（无触发编码）。
    pub fn add_from_text(&mut self, text: &str) -> bool {
        let content = text.trim();
        if content.is_empty() {
            return false;
        }
        let db = xime_config::clipboard_store::default_db_path();
        if xime_config::clipboard_store::add_quick_send(&db, content, "").is_err() {
            return false;
        }
        self.reload();
        true
    }
}

// ------------------------------------------------------------------
// rime 用户资料同步（sync_user_data：用户词典快照导出/合并，多端互通基础）
// ------------------------------------------------------------------

/// rime 用户资料同步状态：本机标识 + sync 快照目录概况（对齐 weasel
/// 「用户资料同步」语义；快照目录可整体上云实现多端词库合并）。
#[cfg(feature = "backup-page")]
#[derive(Clone)]
pub struct RimeSyncState {
    /// installation.yaml 的 installation_id（各端快照目录的隔离键）。
    pub installation_id: String,
    /// 同步快照目录（<rime>/sync）。
    pub sync_dir: std::path::PathBuf,
    /// sync 目录下的设备 installation 列表（含本机；从未同步为空）。
    pub devices: Vec<String>,
    /// 上次同步（sync 目录修改时间的相对描述；从未同步为 None）。
    pub last_sync: Option<String>,
}

#[cfg(feature = "backup-page")]
impl RimeSyncState {
    pub fn load() -> Self {
        let (_, rime_dir) = get_data_dirs();
        let sync_dir = rime_dir.join("sync");
        // installation.yaml 形如 `installation_id: "uuid"`（引号可有可无）。
        let installation_id = std::fs::read_to_string(rime_dir.join("installation.yaml"))
            .ok()
            .and_then(|content| {
                content.lines().find_map(|line| {
                    let rest = line.trim().strip_prefix("installation_id:")?;
                    let v = rest.trim().trim_matches('"').trim_matches('\'').to_string();
                    (!v.is_empty()).then_some(v)
                })
            })
            .unwrap_or_default();
        let mut devices: Vec<String> = std::fs::read_dir(&sync_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        devices.sort();
        let last_sync = std::fs::metadata(&sync_dir)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(relative_time_ago);
        Self {
            installation_id,
            sync_dir,
            devices,
            last_sync,
        }
    }

    pub fn reload(&mut self) {
        *self = Self::load();
    }
}

/// 距今时长 → 人话（刚刚 / N 分钟前 / N 小时前 / N 天前）。
#[cfg(feature = "backup-page")]
fn relative_time_ago(elapsed: std::time::Duration) -> String {
    let mins = elapsed.as_secs() / 60;
    if mins < 1 {
        "刚刚".to_string()
    } else if mins < 60 {
        format!("{mins} 分钟前")
    } else if mins < 60 * 24 {
        format!("{} 小时前", mins / 60)
    } else {
        format!("{} 天前", mins / (60 * 24))
    }
}

// ------------------------------------------------------------------
// 语音转文本（Windows WinRT 听写；speech.rs worker 的 UI 侧镜像）
// ------------------------------------------------------------------

/// 语音转文本页面状态：worker 句柄 + 结果槽镜像（250ms 轮询刷新）。
#[cfg(all(feature = "voice-page", windows))]
#[derive(Clone, Default)]
pub struct SpeechState {
    /// 语音 worker 句柄（首次开始时创建；None = 从未启动）。
    handle: Option<crate::speech::VoiceHandle>,
    /// 正在听写（Listening）。
    pub listening: bool,
    /// 引擎处理中（启动/约束编译）。
    pub processing: bool,
    /// 累计识别文本（镜像 sink.text）。
    pub text: String,
    /// 最近一次错误。
    pub error: Option<String>,
}

#[cfg(all(feature = "voice-page", windows))]
impl SpeechState {
    /// BackgroundPoll 节拍：把 worker 结果槽镜像到 UI 状态。
    pub fn poll(&mut self) {
        let Some(h) = &self.handle else {
            return;
        };
        let sink = h.sink.lock().unwrap_or_else(|e| e.into_inner());
        self.text = sink.text.clone();
        self.listening = sink.state == crate::speech::RecognitionState::Listening;
        self.processing = sink.state == crate::speech::RecognitionState::Processing;
        if sink.state == crate::speech::RecognitionState::Error {
            self.error = sink.error.clone();
        }
    }

    /// 开始/停止听写（worker 首次使用时创建）。
    pub fn toggle(&mut self) {
        let h = self.handle.get_or_insert_with(crate::speech::VoiceHandle::spawn);
        if self.listening || self.processing {
            h.stop();
        } else {
            self.error = None;
            h.start();
        }
    }

    /// 清空累计文本（连同 worker 槽）。
    pub fn clear(&mut self) {
        if let Some(h) = &self.handle {
            let mut sink = h.sink.lock().unwrap_or_else(|e| e.into_inner());
            sink.text.clear();
        }
        self.text.clear();
    }

    /// 复制识别文本到系统剪贴板。
    pub fn copy_text(&self) -> bool {
        if self.text.is_empty() {
            return false;
        }
        match arboard::Clipboard::new() {
            Ok(mut clip) => clip.set_text(self.text.clone()).is_ok(),
            Err(_) => false,
        }
    }
}

// ------------------------------------------------------------------
// 词典管理（用户词典列表 + 备份/恢复/导出/导入，对齐 weasel DictManagementDialog）
// ------------------------------------------------------------------

/// 词典后台任务结果（后台线程 → UI 轮询）。
#[cfg(windows)]
enum DictTaskResult {
    List(DictListResult),
    Message(String),
}

#[cfg(windows)]
static DICT_TASK_OUTCOME: std::sync::Mutex<Option<DictTaskResult>> = std::sync::Mutex::new(None);

/// 词典管理页面状态。
#[cfg(windows)]
#[derive(Clone, Default)]
pub struct DictManageState {
    /// 用户词典名列表。
    pub dicts: Vec<String>,
    /// 快照目录（同步目录）。
    pub sync_dir: String,
    /// 进行中的操作标识（refresh/backup/restore/export/import）。
    pub busy: Option<&'static str>,
    /// 最近一次操作的结果消息。
    pub message: Option<String>,
}

#[cfg(windows)]
impl DictManageState {
    fn submit(message: DictTaskResult) {
        *DICT_TASK_OUTCOME.lock().unwrap_or_else(|e| e.into_inner()) = Some(message);
    }

    /// 刷新用户词典列表。
    pub fn start_refresh(&mut self) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("refresh");
        std::thread::spawn(|| {
            let result = match notify_dict_list() {
                Some(r) => DictTaskResult::List(r),
                None => DictTaskResult::Message(
                    "获取词典列表失败（输入法服务未运行？）".to_string(),
                ),
            };
            Self::submit(result);
        });
    }

    /// 备份词典快照到同步目录。
    pub fn start_backup(&mut self, dict: String) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("backup");
        std::thread::spawn(move || {
            let ok = notify_dict_backup(&dict);
            let msg = if ok {
                format!("已备份 {dict} 快照到同步目录")
            } else {
                format!("备份 {dict} 失败（输入法服务未运行？）")
            };
            Self::submit(DictTaskResult::Message(msg));
        });
    }

    /// 从快照文件恢复。
    pub fn start_restore(&mut self, path: String) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("restore");
        std::thread::spawn(move || {
            let ok = notify_dict_restore(&path);
            let msg = if ok {
                "已从快照恢复用户词典".to_string()
            } else {
                "恢复用户词典失败（快照无效或服务未运行？）".to_string()
            };
            Self::submit(DictTaskResult::Message(msg));
        });
    }

    /// 导出词典为文本。
    pub fn start_export(&mut self, dict: String, path: String) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("export");
        std::thread::spawn(move || {
            let msg = match notify_dict_export(&dict, &path) {
                Some(count) => format!("已导出 {dict}（{count} 条）"),
                None => format!("导出 {dict} 失败（输入法服务未运行？）"),
            };
            Self::submit(DictTaskResult::Message(msg));
        });
    }

    /// 从文本导入词典（合并去重）。
    pub fn start_import(&mut self, dict: String, path: String) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("import");
        std::thread::spawn(move || {
            let msg = match notify_dict_import(&dict, &path) {
                Some(count) => format!("已导入 {dict}（{count} 条）"),
                None => format!("导入 {dict} 失败（文本格式无效或服务未运行？）"),
            };
            Self::submit(DictTaskResult::Message(msg));
        });
    }

    /// BackgroundPoll 节拍：取后台任务结果。
    pub fn poll(&mut self) {
        let outcome = DICT_TASK_OUTCOME
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        match outcome {
            Some(DictTaskResult::List(r)) => {
                self.dicts = r.dicts;
                self.sync_dir = r.sync_dir;
                self.busy = None;
            }
            Some(DictTaskResult::Message(m)) => {
                self.busy = None;
                self.message = Some(m);
            }
            None => {}
        }
    }
}

/// 剪贴板同步插件的页面状态（开关 + 选择 + 配置表单）。
#[cfg(feature = "clipboard-page")]
#[derive(Clone)]
pub struct SyncPluginUiState {
    /// 是否启用插件同步（写入 clipboard_sync.toml，IME 常驻循环热加载）。
    pub enabled: bool,
    /// 已安装的 clipboard_sync 插件。
    pub plugins: Vec<ClipboardSyncPluginInfo>,
    /// 当前选中的插件下标。
    pub plugin_index: usize,
    /// 插件配置表单（schema + 当前值）。
    pub fields: Vec<(xime_plugin::SettingField, String)>,
    pub message: Option<String>,
    pub busy: Option<&'static str>,
}

#[cfg(feature = "clipboard-page")]
static SYNC_PLUGIN_OUTCOME: std::sync::Mutex<Option<Result<SyncPluginOutcome, String>>> =
    std::sync::Mutex::new(None);

#[cfg(feature = "clipboard-page")]
enum SyncPluginOutcome {
    SchemaLoaded(Vec<xime_plugin::SettingField>, Vec<(String, String)>),
    Tested,
}

#[cfg(feature = "clipboard-page")]
impl Default for SyncPluginUiState {
    fn default() -> Self {
        let plugins = scan_clipboard_sync_plugins();
        let cfg = read_clipboard_sync_config();
        let plugin_index = plugins
            .iter()
            .position(|p| p.id == cfg.plugin_id)
            .unwrap_or(0);
        let mut st = Self {
            enabled: cfg.enabled,
            plugins,
            plugin_index,
            fields: Vec::new(),
            message: None,
            busy: None,
        };
        st.start_schema_load();
        st
    }
}

#[cfg(feature = "clipboard-page")]
impl SyncPluginUiState {
    fn selected(&self) -> Option<&ClipboardSyncPluginInfo> {
        self.plugins.get(self.plugin_index)
    }

    /// 后台加载当前插件的 schema 与配置值。
    fn start_schema_load(&mut self) {
        let Some((id, dir)) = self.selected().map(|i| (i.id.clone(), i.dir.clone())) else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("schema");
        self.fields.clear();
        std::thread::spawn(move || {
            let result = load_plugin_runtime(&dir, &id).map(|rt| {
                let fields = rt.get_settings_schema();
                let content = std::fs::read_to_string(
                    config_base_dir()
                        .join("plugins/config")
                        .join(format!("{id}.yaml")),
                )
                .unwrap_or_default();
                let mut config: std::collections::BTreeMap<String, String> =
                    serde_yaml::from_str(&content).unwrap_or_default();
                let key_path = xime_plugin::cipher::key_path_for_config(
                    &config_base_dir()
                        .join("plugins/config")
                        .join(format!("{id}.yaml")),
                );
                for value in config.values_mut() {
                    if let Some(plain) =
                        xime_plugin::cipher::decrypt_with_key_path(&key_path, value)
                    {
                        *value = plain;
                    }
                }
                let values = fields
                    .iter()
                    .map(|f| {
                        (
                            f.key.clone(),
                            config.get(&f.key).cloned().unwrap_or_default(),
                        )
                    })
                    .collect();
                SyncPluginOutcome::SchemaLoaded(fields, values)
            });
            *SYNC_PLUGIN_OUTCOME.lock().unwrap() = Some(result);
        });
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        let mut cfg = read_clipboard_sync_config();
        cfg.enabled = enabled;
        write_clipboard_sync_config(&cfg);
        notify_daemon_reload_plugins();
        self.message = Some(if enabled {
            "插件同步已启用".to_string()
        } else {
            "插件同步已停用".to_string()
        });
    }

    pub fn select(&mut self, index: usize) {
        if index >= self.plugins.len() || index == self.plugin_index {
            return;
        }
        self.plugin_index = index;
        self.message = None;
        let id = self.plugins[index].id.clone();
        let mut cfg = read_clipboard_sync_config();
        cfg.plugin_id = id.clone();
        write_clipboard_sync_config(&cfg);
        // 对齐 Android：选中的同步插件自动启用（daemon 仅加载 plugin_id 一项）。
        let manager = xime_plugin::PluginManager::new(config_base_dir().join("plugins"));
        if let Err(e) = manager.set_enabled(&id, true) {
            let not_found = matches!(
                e,
                xime_plugin::manager::ManagerError::Io(ref io)
                    if io.kind() == std::io::ErrorKind::NotFound
            );
            // 无注册记录的目录直装插件默认视为启用，NotFound 可忽略。
            if !not_found {
                self.message = Some(format!("启用插件失败: {e}"));
            }
        }
        notify_daemon_reload_plugins();
        self.start_schema_load();
    }

    pub fn set_field(&mut self, key: String, value: String) {
        let Some(id) = self.selected().map(|i| i.id.clone()) else {
            return;
        };
        for (field, current) in self.fields.iter_mut() {
            if field.key == key {
                *current = value.clone();
            }
        }
        let path = config_base_dir()
            .join("plugins/config")
            .join(format!("{id}.yaml"));
        let existing: std::collections::BTreeMap<String, String> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_yaml::from_str(&c).ok())
            .unwrap_or_default();
        let mut config = existing;
        config.insert(key, value);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(yaml) = serde_yaml::to_string(&config) {
            let _ = std::fs::write(path, yaml);
        }
        // 配置保存 ≠ 启用同步：开关未开时明确提示，避免「配了但不推送」的困惑。
        if !self.enabled {
            self.message =
                Some("配置已保存；当前同步未启用，请先打开「启用剪贴板同步」开关".to_string());
        }
    }

    pub fn start_test(&mut self) {
        let Some((id, dir)) = self.selected().map(|i| (i.id.clone(), i.dir.clone())) else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("test");
        self.message = None;
        std::thread::spawn(move || {
            let result = load_plugin_runtime(&dir, &id).and_then(|rt| match rt.test_connection() {
                Some(msg) => Err(msg),
                None => Ok(SyncPluginOutcome::Tested),
            });
            *SYNC_PLUGIN_OUTCOME.lock().unwrap() = Some(result);
        });
    }

    /// 轮询后台结果。
    pub fn poll(&mut self) {
        let outcome = SYNC_PLUGIN_OUTCOME.lock().unwrap().take();
        let Some(outcome) = outcome else {
            return;
        };
        self.busy = None;
        match outcome {
            Ok(SyncPluginOutcome::SchemaLoaded(fields, values)) => {
                let map: std::collections::HashMap<String, String> = values.into_iter().collect();
                self.fields = fields
                    .into_iter()
                    .map(|f| {
                        let v = map.get(&f.key).cloned().unwrap_or_default();
                        (f, v)
                    })
                    .collect();
            }
            Ok(SyncPluginOutcome::Tested) => {
                self.message = Some("连接成功".to_string());
            }
            Err(e) => {
                self.message = Some(e);
            }
        }
    }
}

/// 生成随机认证密码（16 字节随机 → URL 安全 base64）。
#[cfg(feature = "clipboard-page")]
fn random_password() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("getrandom");
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

// ---- 云备份（backup-page） --------------------------------------------------

/// 备份提供者：内置 WebDAV 或 backup 类型插件（与 Android 同契约）。
#[cfg(feature = "backup-page")]
#[derive(Clone, Debug, PartialEq)]
pub enum BackupProvider {
    Builtin,
    Plugin {
        id: String,
        name: String,
        dir: std::path::PathBuf,
    },
}

#[cfg(feature = "backup-page")]
impl BackupProvider {
    pub fn label(&self) -> &str {
        match self {
            Self::Builtin => "内置 WebDAV",
            Self::Plugin { name, .. } => name,
        }
    }
}

/// 远端备份列表条目（内置/插件统一展示模型）。
#[cfg(feature = "backup-page")]
#[derive(Clone, Debug, PartialEq)]
pub struct BackupEntry {
    /// 操作 id：内置 = 服务器路径；插件 = 插件定义的 id。
    pub id: String,
    pub name: String,
    pub size: Option<i64>,
}

/// 云备份页状态：提供者选择 + WebDAV（内置）配置 + 备份模式 + 后台操作进度。
#[cfg(feature = "backup-page")]
#[derive(Clone)]
pub struct BackupState {
    /// 可用提供者（内置 WebDAV 恒在，后接 backup 插件）。
    pub providers: Vec<BackupProvider>,
    /// 当前选中的提供者下标。
    pub provider: usize,
    /// 插件配置表单字段（schema + 当前值），选中插件提供者后由后台线程填充。
    pub plugin_fields: Vec<(xime_plugin::SettingField, String)>,
    /// WebDAV 服务器地址（仅内置提供者使用）。
    pub url: String,
    pub username: String,
    /// WebDAV 密码（写入本地配置文件，权限 0600）。
    pub password: String,
    /// 远端备份目录（相对 WebDAV 根路径）。
    pub remote_dir: String,
    /// 备份模式（0=仅配置, 1=全量）。
    pub mode: u8,
    /// 进行中的操作标识（test/schema/backup/list/restore/delete）。
    pub busy: Option<&'static str>,
    /// 最近一次操作的状态消息。
    pub message: Option<String>,
    /// 远端备份列表（"查看远端备份"后填充，切换提供者时清空）。
    pub remote: Vec<BackupEntry>,
    /// 配置文件路径（backup.toml，仅内置提供者的连接信息）。
    config_path: std::path::PathBuf,
}

/// 云备份后台操作的完成结果（线程 → 轮询回 UI）。
#[cfg(feature = "backup-page")]
enum BackupOutcome {
    Tested,
    SchemaLoaded(Vec<xime_plugin::SettingField>, Vec<(String, String)>),
    BackedUp(String),
    Listed(Vec<BackupEntry>),
    Restored(usize),
    Deleted,
}

#[cfg(feature = "backup-page")]
static BACKUP_OUTCOME: std::sync::Mutex<Option<Result<BackupOutcome, String>>> =
    std::sync::Mutex::new(None);

#[cfg(feature = "backup-page")]
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct BackupConfigFile {
    webdav: BackupWebDavSection,
}

#[cfg(feature = "backup-page")]
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct BackupWebDavSection {
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
    remote_dir: Option<String>,
}

#[cfg(any(feature = "backup-page", feature = "clipboard-page"))]
fn backup_plugins_root() -> std::path::PathBuf {
    config_base_dir().join("plugins")
}

/// 扫描 backup 类型且启用的插件（顺序按目录名，稳定）。
#[cfg(feature = "backup-page")]
fn scan_backup_plugins() -> Vec<BackupProvider> {
    let root = backup_plugins_root();
    let manager = xime_plugin::PluginManager::new(&root);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return out;
    };
    let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let Ok(manifest) = xime_plugin::PluginManifest::from_dir(&dir) else {
            continue;
        };
        if manifest.plugin_type() != xime_plugin::PluginType::Backup {
            continue;
        }
        let enabled = manager.get(&manifest.id).map(|r| r.enabled).unwrap_or(true);
        if !enabled {
            continue;
        }
        out.push(BackupProvider::Plugin {
            id: manifest.id,
            name: manifest.name,
            dir,
        });
    }
    out
}

/// 插件配置文件（host.config 同一文件：plugins/config/<id>.yaml）。
#[cfg(any(feature = "backup-page", feature = "clipboard-page"))]
fn plugin_config_path(id: &str) -> std::path::PathBuf {
    backup_plugins_root()
        .join("config")
        .join(format!("{id}.yaml"))
}

#[cfg(any(feature = "backup-page", feature = "clipboard-page"))]
fn read_plugin_config(id: &str) -> std::collections::BTreeMap<String, String> {
    let path = plugin_config_path(id);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Default::default();
    };
    let mut map: std::collections::BTreeMap<String, String> =
        serde_yaml::from_str(&content).unwrap_or_default();
    // 值解密（host.config 同一密文格式；无前缀旧版明文原样）
    let key_path = xime_plugin::cipher::key_path_for_config(&path);
    map.retain(|k, v| match xime_plugin::cipher::decrypt_with_key_path(&key_path, v) {
        Some(plain) => {
            *v = plain;
            true
        }
        None => {
            tracing::warn!("[config] 值解密失败，条目按缺失处理: {k}");
            false
        }
    });
    map
}

#[cfg(any(feature = "backup-page", feature = "clipboard-page"))]
fn write_plugin_config(id: &str, map: &std::collections::BTreeMap<String, String>) {
    let path = plugin_config_path(id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // 值加密落盘（与 host.config 同一密文格式；明文旧值随全量写入自动升级）
    let key_path = xime_plugin::cipher::key_path_for_config(&path);
    let encrypted: std::collections::BTreeMap<String, String> = map
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                xime_plugin::cipher::encrypt_with_key_path(&key_path, v),
            )
        })
        .collect();
    if let Ok(yaml) = serde_yaml::to_string(&encrypted) {
        let _ = std::fs::write(path, yaml);
    }
}

/// 按需加载插件运行时（一操作一实例，onLoad 幂等）。
#[cfg(any(feature = "backup-page", feature = "clipboard-page"))]
fn load_plugin_runtime(
    dir: &std::path::Path,
    id: &str,
) -> Result<xime_plugin::PluginRuntime, String> {
    let manifest = xime_plugin::PluginManifest::from_dir(dir)
        .map_err(|e| format!("读取 manifest 失败: {e}"))?;
    let runtime = xime_plugin::PluginRuntime::load(dir, &manifest.entry, &plugin_config_path(id))
        .map_err(|e| format!("加载插件失败: {e}"))?;
    runtime.call_on_load();
    Ok(runtime)
}

#[cfg(feature = "backup-page")]
impl Default for BackupState {
    fn default() -> Self {
        let providers = scan_backup_plugins();
        let mut st = Self {
            providers,
            provider: 0,
            plugin_fields: Vec::new(),
            url: String::new(),
            username: String::new(),
            password: String::new(),
            remote_dir: "xime-backup".to_string(),
            mode: 0,
            busy: None,
            message: None,
            remote: Vec::new(),
            config_path: config_base_dir().join("backup.toml"),
        };
        st.read_config();
        st
    }
}

#[cfg(feature = "backup-page")]
impl BackupState {
    fn read_config(&mut self) {
        let Ok(content) = std::fs::read_to_string(&self.config_path) else {
            return;
        };
        let Ok(cfg) = toml::from_str::<BackupConfigFile>(&content) else {
            return;
        };
        if let Some(v) = cfg.webdav.url {
            self.url = v;
        }
        if let Some(v) = cfg.webdav.username {
            self.username = v;
        }
        if let Some(v) = cfg.webdav.password {
            self.password = v;
        }
        if let Some(v) = cfg.webdav.remote_dir {
            self.remote_dir = v;
        }
    }

    /// 保存内置 WebDAV 配置（0600，密码仅本地可读）。
    fn write_config(&self) -> Result<(), String> {
        let cfg = BackupConfigFile {
            webdav: BackupWebDavSection {
                url: Some(self.url.clone()),
                username: Some(self.username.clone()),
                password: Some(self.password.clone()),
                remote_dir: Some(self.remote_dir.clone()),
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

    fn client(&self) -> Result<crate::webdav::WebDavClient, String> {
        if self.url.trim().is_empty() {
            return Err("请先填写 WebDAV 服务器地址".to_string());
        }
        Ok(crate::webdav::WebDavClient::new(
            self.url.trim().to_string(),
            (!self.username.is_empty()).then_some(self.username.clone()),
            (!self.password.is_empty()).then_some(self.password.clone()),
        ))
    }

    /// 当前提供者是否为插件。
    pub fn is_plugin_selected(&self) -> bool {
        matches!(
            self.providers.get(self.provider),
            Some(BackupProvider::Plugin { .. })
        )
    }

    /// 切换提供者：清空列表；插件提供者则后台加载 schema + 配置值。
    pub fn select_provider(&mut self, index: usize) {
        if index >= self.providers.len() || index == self.provider {
            return;
        }
        self.provider = index;
        self.remote.clear();
        self.plugin_fields.clear();
        self.message = None;
        let Some(BackupProvider::Plugin { id, dir, .. }) = self.providers.get(index) else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("schema");
        let id = id.clone();
        let dir = dir.clone();
        std::thread::spawn(move || {
            let result = load_plugin_runtime(&dir, &id).map(|rt| {
                let fields = rt.get_settings_schema();
                let config = read_plugin_config(&id);
                let values = fields
                    .iter()
                    .map(|f| {
                        (
                            f.key.clone(),
                            config.get(&f.key).cloned().unwrap_or_default(),
                        )
                    })
                    .collect();
                BackupOutcome::SchemaLoaded(fields, values)
            });
            *BACKUP_OUTCOME.lock().unwrap() = Some(result);
        });
    }

    /// 更新插件配置字段（写 host.config 同一文件，插件立即可见）。
    pub fn set_plugin_field(&mut self, key: String, value: String) {
        let Some(BackupProvider::Plugin { id, .. }) = self.providers.get(self.provider) else {
            return;
        };
        let id = id.clone();
        for (field, current) in self.plugin_fields.iter_mut() {
            if field.key == key {
                *current = value.clone();
            }
        }
        let mut config = read_plugin_config(&id);
        config.insert(key, value);
        write_plugin_config(&id, &config);
    }

    /// 通用后台任务启动：守卫 busy → 落盘配置 → 开线程执行 → 结果进轮询槽。
    fn start_op(
        &mut self,
        tag: &'static str,
        op: impl FnOnce(&BackupState) -> Result<BackupOutcome, String> + Send + 'static,
    ) {
        if self.busy.is_some() {
            return;
        }
        if let Err(e) = self.write_config() {
            self.message = Some(format!("保存配置失败: {e}"));
            return;
        }
        self.busy = Some(tag);
        self.message = None;
        let snapshot = self.clone();
        std::thread::spawn(move || {
            let result = op(&snapshot);
            *BACKUP_OUTCOME.lock().unwrap() = Some(result);
        });
    }

    pub fn start_test(&mut self) {
        self.start_op("test", |state| match state.providers.get(state.provider) {
            Some(BackupProvider::Plugin { id, dir, .. }) => {
                let runtime = load_plugin_runtime(dir, id)?;
                if let Some(msg) = runtime.test_connection() {
                    return Err(msg);
                }
                Ok(BackupOutcome::Tested)
            }
            _ => {
                state.client()?.test()?;
                Ok(BackupOutcome::Tested)
            }
        });
    }

    pub fn start_backup(&mut self) {
        self.start_op("backup", |state| {
            let mode = crate::backup::BackupMode::from_index(state.mode).ok_or("备份模式无效")?;
            let (_, user_dir) = get_data_dirs();
            let archive = crate::backup::pack_rime(&user_dir, mode)?;
            let name = crate::backup::archive_name(
                mode,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_secs(),
            );
            match state.providers.get(state.provider) {
                Some(BackupProvider::Plugin { id, dir, .. }) => {
                    let runtime = load_plugin_runtime(dir, id)?;
                    let result = runtime.push_backup(&name, &archive);
                    if !result.ok {
                        return Err(result.message.unwrap_or_else(|| "上传失败".to_string()));
                    }
                }
                _ => {
                    let key = format!("{}/{}", state.remote_dir.trim_matches('/'), name);
                    state.client()?.put(&key, &archive)?;
                }
            }
            Ok(BackupOutcome::BackedUp(name))
        });
    }

    pub fn start_list(&mut self) {
        self.start_op("list", |state| match state.providers.get(state.provider) {
            Some(BackupProvider::Plugin { id, dir, .. }) => {
                let runtime = load_plugin_runtime(dir, id)?;
                let list = runtime.list_backups().ok_or("获取备份列表失败")?;
                Ok(BackupOutcome::Listed(
                    list.into_iter()
                        .map(|e| BackupEntry {
                            id: e.id,
                            name: e.name,
                            size: Some(e.size),
                        })
                        .collect(),
                ))
            }
            _ => {
                let dir = state.remote_dir.trim_matches('/').to_string();
                Ok(BackupOutcome::Listed(
                    state
                        .client()?
                        .list(&dir)?
                        .into_iter()
                        .map(|f| BackupEntry {
                            id: f.path,
                            name: f.name,
                            size: f.size.map(|s| s as i64),
                        })
                        .collect(),
                ))
            }
        });
    }

    pub fn start_restore(&mut self, id: String) {
        self.start_op("restore", move |state| {
            let data = match state.providers.get(state.provider) {
                Some(BackupProvider::Plugin { id: pid, dir, .. }) => {
                    let runtime = load_plugin_runtime(dir, pid)?;
                    runtime
                        .pull_backup(&id)
                        .ok_or_else(|| "下载备份包失败".to_string())?
                }
                _ => state
                    .client()?
                    .get(&id)?
                    .ok_or_else(|| "远端备份不存在".to_string())?,
            };
            let (_, user_dir) = get_data_dirs();
            let n = crate::backup::unpack_rime(&data, &user_dir)?;
            Ok(BackupOutcome::Restored(n))
        });
    }

    pub fn start_delete(&mut self, id: String) {
        self.start_op("delete", move |state| {
            match state.providers.get(state.provider) {
                Some(BackupProvider::Plugin { id: pid, dir, .. }) => {
                    let runtime = load_plugin_runtime(dir, pid)?;
                    if !runtime.delete_backup(&id) {
                        return Err("删除失败".to_string());
                    }
                }
                _ => {
                    state.client()?.delete(&id)?;
                }
            }
            Ok(BackupOutcome::Deleted)
        });
    }

    /// 轮询后台结果（由 poll_background 调用）。
    pub fn poll(&mut self) {
        let outcome = BACKUP_OUTCOME.lock().unwrap().take();
        let Some(outcome) = outcome else {
            return;
        };
        self.busy = None;
        match outcome {
            Ok(BackupOutcome::Tested) => {
                self.message = Some("连接成功".to_string());
            }
            Ok(BackupOutcome::SchemaLoaded(fields, values)) => {
                let map: std::collections::HashMap<String, String> = values.into_iter().collect();
                self.plugin_fields = fields
                    .into_iter()
                    .map(|f| {
                        let v = map.get(&f.key).cloned().unwrap_or_default();
                        (f, v)
                    })
                    .collect();
            }
            Ok(BackupOutcome::BackedUp(name)) => {
                self.message = Some(format!("备份完成：{name}"));
            }
            Ok(BackupOutcome::Listed(files)) => {
                self.message = Some(format!("共 {} 个远端备份", files.len()));
                self.remote = files;
            }
            Ok(BackupOutcome::Restored(n)) => {
                self.message = Some(format!("恢复完成（{n} 个文件），重启输入法后生效"));
            }
            Ok(BackupOutcome::Deleted) => {
                self.message = Some("已删除".to_string());
                // 删除后自动刷新列表。
                self.start_list();
            }
            Err(e) => {
                self.message = Some(e);
            }
        }
    }
}

// ---- 云备份（backup-page）结束 ----------------------------------------------

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
    let mut response = http_client()
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| anyhow::anyhow!("网络请求失败: {}", e))?;
    let total = response.content_length().unwrap_or(0);

    let mut hasher = sha256.map(|_| sha2::Sha256::new());
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| anyhow::anyhow!("创建目录失败: {}", e))?;
    }
    let mut output =
        std::fs::File::create(dest).map_err(|e| anyhow::anyhow!("创建文件失败: {}", e))?;

    let mut buf = [0u8; 8192];
    let mut done: u64 = 0;
    loop {
        let n = response
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

    /// v2 插件子索引（JS/QuickJS 插件）的裁剪样本——字段形态取自
    /// https://index.ximei.me/plugins/v2/index.yaml。
    const PLUGIN_INDEX_V2_YAML: &str = r#"
index_version: 2
updated_at: '2026-09-26'
plugins:
- id: com.kingzcheung.xime.plugin.ai_reply
  name: AI 智能回复
  author: Xime
  description: 根据对方消息生成回复候选。
  type: remote
  tags:
  - AI
  pluginType: tool
  icon: 应
  activation: multi
  minHostVersion: 3.0.0
  platforms:
  - android
  capabilities:
    tool:
      display: passive
  network:
    allowCustomHosts: true
  homepage: https://example.com
  license: GPL-3.0
  appVersion: '>=3.0.0'
  currentVersion: 1.0.0
  versions:
  - version: 1.0.0
    date: '2026-09-24'
    changelog: v3 重构：TypeScript/QuickJS 插件
    downloadUrl:
    - url: https://example.com/ai-reply-1.0.0.xipk
      sha256: 4e5454210000000000000000000000000000000000000000000000000000000
      size: 3.4 KB
      sizeBytes: 3469
"#;

    #[test]
    fn parse_plugin_index_v2() {
        let index: PluginIndex = serde_yaml::from_str(PLUGIN_INDEX_V2_YAML).unwrap();
        assert_eq!(index.index_version, 2);
        assert_eq!(index.updated_at, "2026-09-26");
        assert_eq!(index.plugins.len(), 1);

        let plugin = &index.plugins[0];
        assert_eq!(plugin.id, "com.kingzcheung.xime.plugin.ai_reply");
        assert_eq!(plugin.plugin_kind, "tool", "pluginType 字段应被解析");
        assert_eq!(plugin.current_version.as_deref(), Some("1.0.0"));

        let version = &plugin.versions[0];
        assert_eq!(version.version, "1.0.0");
        assert_eq!(version.download_url.len(), 1);
        assert_eq!(
            version.download_url[0].url,
            "https://example.com/ai-reply-1.0.0.xipk"
        );
        assert!(
            version.download_url[0].sha256.is_some(),
            "sha256 用于下载校验"
        );
    }

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

    /// 列表翻页：总页数、末页不前进、越界夹回、条目缩减后夹回。
    /// （不触库：直接构造状态，历史与快捷发送共用同一套页逻辑）
    #[cfg(feature = "clipboard-page")]
    #[test]
    fn list_pagination_pages_and_clamps() {
        let mk = |n: usize| ClipboardHistoryState {
            items: (0..n)
                .map(|i| xime_config::clipboard_store::ClipboardHistoryItem {
                    id: i as i64,
                    text: format!("t{i}"),
                    timestamp: 0,
                })
                .collect(),
            selected: None,
            page: 0,
        };

        // 17 条 → 3 页
        let mut h = mk(17);
        assert_eq!(h.total_pages(), 3);
        h.next_page();
        h.next_page();
        h.next_page(); // 已在末页，不再前进
        assert_eq!(h.page, 2);
        h.prev_page();
        assert_eq!(h.page, 1);
        // 空列表至少 1 页
        assert_eq!(mk(0).total_pages(), 1);

        // 条目缩减后页码夹回有效范围
        h.page = 9;
        h.clamp_page();
        assert_eq!(h.page, 2);
        h.items.truncate(8);
        h.clamp_page();
        assert_eq!(h.page, 0);
    }
}
