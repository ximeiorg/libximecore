pub mod rime_deploy;
pub mod schema_config;
pub mod schema_manager;
pub mod smart_suggestion;
pub mod style;
pub mod wubi_radicals;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing_subscriber::prelude::*;

pub use rime_deploy::{
    deploy_all, deploy_all_schemas, get_data_dirs, init_rime_deployer, SchemaInfo,
};
pub use schema_config::{
    ReverseLookupConfig, SchemaConfig, SchemaConfigManager, SpellerConfig, TraditionConfig,
    TranslatorConfig,
};
pub use schema_manager::SchemaManager;
pub use smart_suggestion::{
    SmartSuggestionConfig, SmartSuggestionModelConfig, SmartSuggestionModelFile,
};
pub use style::ColorScheme;
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
    pub smart_suggestion: SmartSuggestionConfig,
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

        // Linux: /usr/share/xime/xime.yaml
        if cfg!(unix) {
            paths.push(PathBuf::from("/usr/share/xime/xime.yaml"));
        }

        // Windows: data/xime.yaml next to exe
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                paths.push(parent.join("data").join("xime.yaml"));
                paths.push(parent.join("resources").join("xime.yaml"));
            }
        }

        paths
    }

    pub fn user_config_path() -> PathBuf {
        // Linux: ~/.config/xime/xime.custom.yaml or xime.yaml
        if cfg!(unix) {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
            let base = PathBuf::from(&home).join(".config/xime");
            for path in &[
                base.join("xime.custom.yaml"),
                base.join("rime/xime.custom.yaml"),
                base.join("xime.yaml"),
                base.join("rime/xime.yaml"),
            ] {
                if path.exists() {
                    return path.clone();
                }
            }
            return base.join("xime.custom.yaml");
        }

        // Windows: %APPDATA%/Xime/rime/xime.yaml
        if let Ok(appdata) = std::env::var("APPDATA") {
            let base = PathBuf::from(&appdata).join("Xime");
            for path in &[
                base.join("rime/xime.custom.yaml"),
                base.join("rime/xime.yaml"),
            ] {
                if path.exists() {
                    return path.clone();
                }
            }
            return base.join("rime/xime.custom.yaml");
        }

        PathBuf::from("xime.custom.yaml")
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
            smart_suggestion: SmartSuggestionConfig {
                enabled: over.smart_suggestion.enabled.or(base.smart_suggestion.enabled),
                suggestion_count: over.smart_suggestion.suggestion_count,
                record_user_frequency: over.smart_suggestion.record_user_frequency,
                auto_adjust_frequency: over.smart_suggestion.auto_adjust_frequency,
                learning_threshold: over.smart_suggestion.learning_threshold,
                model: over.smart_suggestion.model,
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
        let content =
            serde_yaml::to_string(self).map_err(|e| format!("Failed to serialize config: {}", e))?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir: {}", e))?;
        }

        fs::write(&path, content).map_err(|e| format!("Failed to write config: {}", e))?;

        Ok(())
    }

    pub fn get_primary_color(&self) -> (u8, u8, u8) {
        let scheme_name = &self.style.color_scheme;
        if let Some(scheme) = self.color_schemes.get(scheme_name) {
            let r = (scheme.primary_color >> 16) as u8;
            let g = (scheme.primary_color >> 8) as u8;
            let b = scheme.primary_color as u8;
            (r, g, b)
        } else {
            (0x8F, 0x73, 0xE2)
        }
    }
}

// Logging support (cross-platform)
static LOG_GUARD: Mutex<Option<tracing_appender::non_blocking::WorkerGuard>> =
    Mutex::new(None);

pub fn init_logging(component: &str) {
    let log_dir = get_log_dir();
    fs::create_dir_all(&log_dir).ok();

    let file_appender = tracing_appender::rolling::never(
        &log_dir,
        format!("{}.log", component),
    );

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

    let file_appender = tracing_appender::rolling::never(
        &log_dir,
        format!("{}.log", component),
    );

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
            .join("xime")
    } else {
        dirs_or_home().join("log")
    }
}

fn dirs_or_home() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local/share/xime")
    } else {
        PathBuf::from("/tmp/xime")
    }
}
