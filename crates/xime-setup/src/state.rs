use crate::theme::{SystemTheme, ThemeColors};
use serde::Deserialize;
use std::sync::{Mutex, OnceLock};
use xime_config::{
    deploy_all, get_data_dirs, SchemaConfig, SchemaConfigManager, SchemaInfo, SchemaManager,
    XimeConfig,
};

static MARKET_TASK_RESULT: OnceLock<Mutex<Option<MarketTaskResult>>> = OnceLock::new();
static MARKET_YAML_RESULT: OnceLock<Mutex<Option<Result<String, String>>>> = OnceLock::new();
static DEPLOY_RESULT: OnceLock<Mutex<Option<Result<(), String>>>> = OnceLock::new();

fn market_task_result() -> &'static Mutex<Option<MarketTaskResult>> {
    MARKET_TASK_RESULT.get_or_init(|| Mutex::new(None))
}

fn market_yaml_result() -> &'static Mutex<Option<Result<String, String>>> {
    MARKET_YAML_RESULT.get_or_init(|| Mutex::new(None))
}

fn deploy_result() -> &'static Mutex<Option<Result<(), String>>> {
    DEPLOY_RESULT.get_or_init(|| Mutex::new(None))
}

enum MarketTaskResult {
    DownloadDone(String),
    InstallDone(String),
    UninstallDone(String),
    DeleteDone(String),
    Error(String),
}

static NOTIFY_DEPLOY: OnceLock<fn()> = OnceLock::new();
static NOTIFY_RELOAD_STYLE: OnceLock<fn()> = OnceLock::new();
static NOTIFY_SELECT_SCHEMA: OnceLock<fn(&str) -> bool> = OnceLock::new();

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

fn cache_dir() -> std::path::PathBuf {
    let (_, user_data_dir) = get_data_dirs();
    user_data_dir
        .parent()
        .map(|p| p.join("market"))
        .unwrap_or_else(|| {
            let base = std::env::var("LOCALAPPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::env::temp_dir());
            base.join("xime").join("market")
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
    /// 方案市场：加载失败后重试。
    MarketRetry,
    /// 外观：字号变更。
    FontSizeChanged(f64),
    /// 外观：候选词数量变更。
    CandidateCountChanged(i32),
    /// 外观：圆角大小变更。
    CornerRadiusChanged(f64),
    /// 外观：保存。
    SaveAppearance,
    #[cfg(feature = "smart-suggestion-page")]
    SaveSmartSuggestion,
    #[cfg(feature = "clipboard-page")]
    ClearClipboardHistory,
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
    pub deploy_message: Option<String>,
    pub schemas_loaded: bool,
    pub market_schema: MarketSchemaState,
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
            system_theme: SystemTheme::detect(),
            deploy_message: None,
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
        state.start_load_market();
        state
    }

    pub fn colors(&self) -> ThemeColors {
        let primary_color = self.get_primary_color();
        ThemeColors::from_theme(&self.system_theme, primary_color)
    }

    fn get_primary_color(&self) -> u32 {
        self.appearance
            .available_color_schemes
            .iter()
            .find(|(id, _, _)| id == &self.appearance.color_scheme)
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

        if !notify_select_schema(selected_id) {
            return Err(format!(
                "切换输入方案失败：无法向服务器发送 SelectSchema 命令（{}）",
                selected_id
            ));
        }
        Ok(())
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

    pub fn start_deploy(&mut self) {
        if deploy_result().lock().unwrap().is_some() {
            return;
        }
        self.deploy_message = Some("正在部署…".to_string());
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
                    self.deploy_message = Some(if notify_daemon_reload() {
                        "部署成功！配置已重载。".to_string()
                    } else {
                        "部署成功！(服务器未运行，配置将在下次启动时生效)".to_string()
                    });
                }
                Err(e) => {
                    self.deploy_message = Some(format!("部署失败: {}", e));
                }
            }
        }
    }

    // ---- 方案市场 ----

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
            Ok(text) => {
                match serde_yaml::from_str::<SchemaIndex>(&text) {
                    Ok(index) => {
                        self.market_schema.installed_ids = self.get_installed_schema_ids();
                        self.market_schema.downloaded_ids = self.get_cached_schema_ids();
                        self.market_schema.schemas = index.schemas;
                        self.market_schema.loaded = true;
                        self.market_schema.loading = false;
                        self.market_schema.error = None;
                    }
                    Err(e) => {
                        self.market_schema.loading = false;
                        self.market_schema.error = Some(format!("解析失败: {}", e));
                    }
                }
            }
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
            }
        }
        self.market_schema.downloading = None;
        self.market_schema.installing = None;
        true
    }

    /// 统一回收后台任务结果（由轮询订阅调用）。
    pub fn poll_background(&mut self) {
        self.poll_deploy();
        self.poll_market_yaml();
        self.poll_market_task();
    }

    pub fn download_market_schema(&mut self, schema_id: &str) {
        if self.market_schema.downloading.is_some() || self.market_schema.installing.is_some() {
            return;
        }

        let schema = match self.market_schema.schemas.iter().find(|s| s.id == schema_id) {
            Some(s) => s.clone(),
            None => return,
        };

        self.market_schema.downloading = Some(schema_id.to_string());
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
            let pkg_dir = cache_dir().join(&sid);
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
        let dir = cache_dir();
        if !dir.exists() {
            return Vec::new();
        }
        let mut ids = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        ids.push(name.to_string());
                    }
                }
            }
        }
        ids
    }
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
#[derive(Clone, Default)]
pub struct ClipboardState {}

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

#[derive(Clone, Default)]
pub struct AppearanceState {
    pub font_size: f64,
    pub candidate_count: i32,
    pub corner_radius: f64,
    pub color_scheme: String,
    pub available_color_schemes: Vec<(String, String, u32)>,
    pub color_schemes_loaded: bool,
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
    pub installing: Option<String>,
    pub install_message: Option<String>,
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
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

// ---- installation helpers ----

fn get_download_info(schema: &MarketSchema) -> Option<(String, String)> {
    let version = schema.current_version.as_deref().unwrap_or("latest");
    let info = schema
        .versions
        .iter()
        .find(|v| v.version == version || version == "latest")
        .or_else(|| schema.versions.first())?;
    let url = info.download_url.first()?;
    let ext = if url.url.ends_with(".zip") {
        ".zip"
    } else if url.url.ends_with(".tar.gz") {
        ".tar.gz"
    } else {
        return None;
    };
    Some((url.url.clone(), format!("{}{}", version, ext)))
}

fn do_uninstall(schema_id: &str) -> anyhow::Result<()> {
    let (_, user_data_dir) = get_data_dirs();
    let market_dir = cache_dir();
    let registry_path = market_dir.join(".registry.yaml");

    // Read registry to find installed files
    let files_to_remove: Vec<String> = if registry_path.exists() {
        let content = std::fs::read_to_string(&registry_path)?;
        #[derive(serde::Deserialize)]
        struct Entry {
            files: Vec<String>,
        }
        let registry: std::collections::HashMap<String, Entry> =
            serde_yaml::from_str(&content)?;
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
    let (url, filename) = get_download_info(schema)
        .ok_or_else(|| anyhow::anyhow!("无可用下载地址或不支持的格式"))?;

    let dest_dir = cache_dir().join(&schema.id);
    std::fs::create_dir_all(&dest_dir)?;

    let bytes = ureq::get(&url)
        .call()?
        .into_body()
        .read_to_vec()
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    std::fs::write(dest_dir.join(&filename), &bytes)?;
    Ok(())
}

fn do_install(schema_id: &str) -> anyhow::Result<()> {
    let cache_schema_dir = cache_dir().join(schema_id);
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

    let filename = archive_path.file_name().unwrap().to_string_lossy().to_string();
    if filename.ends_with(".zip") {
        extract_zip(&archive_path, &temp_dir)?;
    } else {
        extract_tar_gz(&archive_path, &temp_dir)?;
    }

    let (_, user_data_dir) = get_data_dirs();
    let schema_files = find_schema_files(&temp_dir);
    anyhow::ensure!(!schema_files.is_empty(), "未在下载包中找到 .schema.yaml 文件");

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
    manager
        .save()
        .map_err(|e| anyhow::anyhow!("{}", e))?;

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
