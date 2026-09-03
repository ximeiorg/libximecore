pub mod metadata;
pub mod rime_deploy;
pub mod schema_config;
pub mod schema_manager;
pub mod style;
pub mod wubi_radicals;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing_subscriber::prelude::*;

pub use metadata::{app_metadata, set_app_metadata, AppMetadata};
pub use rime_deploy::{
    default_rime_paths, deploy_all, deploy_all_schemas, get_data_dirs, init_rime_deployer,
    set_rime_paths, RimePaths, SchemaInfo,
};
pub use schema_config::{
    ReverseLookupConfig, SchemaConfig, SchemaConfigManager, SpellerConfig, TraditionConfig,
    TranslatorConfig,
};
pub use schema_manager::SchemaManager;
pub use style::ColorScheme;
pub use style::ColorSchemeConfig;
pub use style::DarkMode;
pub use style::StyleConfig;
pub use wubi_radicals::{KeyRadicalsConfig, WubiRadicalsConfig};

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub struct XimeConfig {
    #[serde(default)]
    pub wubi_radicals: WubiRadicalsConfig,
    #[serde(default)]
    pub style: StyleConfig,
    #[serde(default)]
    pub color_schemes: HashMap<String, ColorScheme>,
    #[serde(default)]
    pub pair_secret: String,
}

impl XimeConfig {
    pub fn load() -> Self {
        // 1. Builtin defaults
        let mut config = Self::builtin_default();

        // 2. System config
        if let Some(system) = Self::load_system_config() {
            config = Self::merge_configs(config, system);
        }

        // 3. User config
        if let Some(user) = Self::load_user_config() {
            config = Self::merge_configs(config, user);
        }

        config
    }

    fn load_system_config() -> Option<Self> {
        for path in Self::system_config_paths() {
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(config) = serde_yaml::from_str::<XimeConfig>(&content) {
                        return Some(config);
                    }
                }
            }
        }
        None
    }

    fn load_user_config() -> Option<Self> {
        let path = Self::user_config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(config) = serde_yaml::from_str::<XimeConfig>(&content) {
                    return Some(config);
                }
            }
        }
        None
    }

    fn builtin_default() -> Self {
        const DEFAULT_CONFIG: &[u8] = include_bytes!("../../../resources/xime.yaml");
        serde_yaml::from_slice(DEFAULT_CONFIG).unwrap_or_default()
    }

    fn system_config_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let meta = app_metadata();
        let config_dir = meta.config_dir_name;
        let config_file = format!("{}.yaml", meta.config_file_base);

        // Linux: /usr/share/xime/xime.yaml
        if cfg!(unix) {
            paths.push(
                PathBuf::from("/usr/share")
                    .join(config_dir)
                    .join(&config_file),
            );
        }

        // Windows: data/xime.yaml next to exe
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                paths.push(parent.join("data").join(&config_file));
                paths.push(parent.join("resources").join(&config_file));
            }
        }

        paths
    }

    pub fn user_config_path() -> PathBuf {
        let meta = app_metadata();
        let config_dir = meta.config_dir_name;
        let custom_file = format!("{}.custom.yaml", meta.config_file_base);
        let config_file = format!("{}.yaml", meta.config_file_base);

        // macOS: ~/Library/Application Support/Xime/xime.custom.yaml
        if cfg!(target_os = "macos") {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
            let base = PathBuf::from(&home)
                .join("Library/Application Support")
                .join(config_dir);
            for path in &[base.join(&custom_file), base.join(&config_file)] {
                if path.exists() {
                    return path.clone();
                }
            }
            return base.join(custom_file);
        }

        // Linux: ~/.config/xime/xime.custom.yaml or xime.yaml
        if cfg!(unix) {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
            let base = PathBuf::from(&home).join(".config").join(config_dir);
            for path in &[
                base.join(&custom_file),
                base.join("rime").join(&custom_file),
                base.join(&config_file),
                base.join("rime").join(&config_file),
            ] {
                if path.exists() {
                    return path.clone();
                }
            }
            return base.join(custom_file);
        }

        // Windows: %APPDATA%/Xime/rime/xime.yaml
        if let Ok(appdata) = std::env::var("APPDATA") {
            let base = PathBuf::from(&appdata).join(config_dir);
            for path in &[
                base.join("rime").join(&custom_file),
                base.join("rime").join(&config_file),
            ] {
                if path.exists() {
                    return path.clone();
                }
            }
            return base.join("rime").join(custom_file);
        }

        PathBuf::from(custom_file)
    }

    fn merge_configs(base: Self, over: Self) -> Self {
        XimeConfig {
            wubi_radicals: WubiRadicalsConfig {
                hotkeys: over.wubi_radicals.hotkeys,
                schema_radicals: if over.wubi_radicals.schema_radicals.is_empty() {
                    base.wubi_radicals.schema_radicals
                } else {
                    over.wubi_radicals.schema_radicals
                },
            },
            style: over.style,
            color_schemes: if over.color_schemes.is_empty() {
                base.color_schemes
            } else {
                over.color_schemes
            },
            pair_secret: if over.pair_secret.is_empty() {
                base.pair_secret
            } else {
                over.pair_secret
            },
        }
    }

    pub fn config_path() -> PathBuf {
        Self::user_config_path()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        let content = serde_yaml::to_string(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir: {}", e))?;
        }

        fs::write(&path, content).map_err(|e| format!("Failed to write config: {}", e))?;

        Ok(())
    }

    pub fn get_primary_color(&self) -> (u8, u8, u8) {
        // Default to light mode
        self.get_primary_color_for_theme(false)
    }

    pub fn get_primary_color_for_theme(&self, system_is_dark: bool) -> (u8, u8, u8) {
        let is_dark = self.style.dark_mode.is_dark(system_is_dark);
        let scheme_name = self.style.color_scheme.scheme_name(is_dark);
        if let Some(scheme) = self.color_schemes.get(&scheme_name) {
            let r = (scheme.primary_color >> 16) as u8;
            let g = (scheme.primary_color >> 8) as u8;
            let b = scheme.primary_color as u8;
            (r, g, b)
        } else {
            (0x8F, 0x73, 0xE2)
        }
    }

    /// Get the resolved color scheme name based on dark_mode setting.
    pub fn get_color_scheme_name(&self, system_is_dark: bool) -> String {
        let is_dark = self.style.dark_mode.is_dark(system_is_dark);
        self.style.color_scheme.scheme_name(is_dark)
    }

    /// Get keyboard background color for the current theme.
    pub fn get_keyboard_bg_color(&self, system_is_dark: bool) -> Option<u32> {
        let is_dark = self.style.dark_mode.is_dark(system_is_dark);
        let scheme_name = self.style.color_scheme.scheme_name(is_dark);
        if let Some(scheme) = self.color_schemes.get(&scheme_name) {
            if let Some(ref bg) = scheme.keyboard_background {
                if is_dark {
                    return bg.color_dark;
                } else {
                    return bg.color;
                }
            }
        }
        None
    }

    /// Get key background color for the current theme.
    pub fn get_key_bg_color(&self, system_is_dark: bool) -> Option<u32> {
        let is_dark = self.style.dark_mode.is_dark(system_is_dark);
        let scheme_name = self.style.color_scheme.scheme_name(is_dark);
        if let Some(scheme) = self.color_schemes.get(&scheme_name) {
            if is_dark {
                return scheme.key_bg_color_dark;
            } else {
                return scheme.key_bg_color;
            }
        }
        None
    }

    /// Get wubi root text for a key press in the given schema.
    /// Delegates to `wubi_radicals`.
    pub fn get_root_for_key(&self, schema: &str, key: char) -> Option<String> {
        self.wubi_radicals.get_root_for_key(schema, key)
    }

    /// Get the key binding that triggers the wubi radicals overlay.
    /// Returns e.g. "Ctrl" or "Shift" from `wubi_radicals.hotkeys.show_key`.
    pub fn get_last_key_root_binding(&self) -> String {
        self.wubi_radicals.hotkeys.show_key.clone()
    }
}

// Logging support (cross-platform)
static LOG_GUARD: Mutex<Option<tracing_appender::non_blocking::WorkerGuard>> = Mutex::new(None);

pub fn init_logging(component: &str) {
    let log_dir = get_log_dir();
    fs::create_dir_all(&log_dir).ok();

    let file_appender = tracing_appender::rolling::never(&log_dir, format!("{}.log", component));

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    if let Ok(mut g) = LOG_GUARD.lock() {
        *g = Some(guard);
    }

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(false)
        .with_line_number(true)
        .try_init()
        .ok();
}

pub fn init_logging_with_console(component: &str) {
    let log_dir = get_log_dir();
    fs::create_dir_all(&log_dir).ok();

    let file_appender = tracing_appender::rolling::never(&log_dir, format!("{}.log", component));

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    if let Ok(mut g) = LOG_GUARD.lock() {
        *g = Some(guard);
    }

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_target(false)
                .with_line_number(true),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout)
                .with_ansi(true),
        )
        .try_init()
        .ok();
}

fn get_log_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::var("TEMP")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(app_metadata().config_dir_name)
    } else {
        dirs_or_home().join("log")
    }
}

fn dirs_or_home() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".local/share")
            .join(app_metadata().config_dir_name)
    } else {
        PathBuf::from("/tmp").join(app_metadata().config_dir_name)
    }
}
