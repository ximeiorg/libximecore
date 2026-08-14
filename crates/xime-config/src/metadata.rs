//! 应用元数据：libximecore 是跨平台共享库，宿主应用（Xime / XimeChe / Luotuo 等）
//! 通过 [`set_app_metadata`] 注入自身名称，未注入时保持默认 "Xime" 兼容行为。

use std::sync::OnceLock;

/// 应用标识信息。所有字段均为 `&'static str`，便于 `const` 构造。
#[derive(Debug, Clone)]
pub struct AppMetadata {
    /// UI 显示名，如 "Xime" / "曦码·澈输入法"。
    pub display_name: &'static str,
    /// 配置目录名（小写），如 "xime"（Linux `~/.config/<name>`、`/usr/share/<name>`）。
    pub config_dir_name: &'static str,
    /// 配置文件基名，如 "xime"（`<base>.yaml`、`<base>.custom.yaml`）。
    pub config_file_base: &'static str,
    /// librime distribution name。
    pub distribution_name: &'static str,
    /// librime distribution code name。
    pub distribution_code_name: &'static str,
    /// librime app name（`rime.<name>.<component>` 惯例）。
    pub app_name: &'static str,
    /// 版本号。
    pub version: &'static str,
}

impl Default for AppMetadata {
    fn default() -> Self {
        Self {
            display_name: "Xime",
            config_dir_name: "xime",
            config_file_base: "xime",
            distribution_name: "Xime",
            distribution_code_name: "Xime",
            app_name: "rime.xime.setup",
            version: "1.0",
        }
    }
}

static APP_METADATA: OnceLock<AppMetadata> = OnceLock::new();

/// 注入应用元数据。必须在首次调用 [`app_metadata`] 之前调用，重复调用返回错误。
pub fn set_app_metadata(meta: AppMetadata) -> Result<(), String> {
    APP_METADATA
        .set(meta)
        .map_err(|_| "app metadata already set".to_string())
}

/// 获取应用元数据；未注入时返回默认值（Xime）。
pub fn app_metadata() -> &'static AppMetadata {
    APP_METADATA.get_or_init(AppMetadata::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_metadata_is_xime() {
        let meta = app_metadata();
        assert_eq!(meta.display_name, "Xime");
        assert_eq!(meta.config_dir_name, "xime");
        assert_eq!(meta.config_file_base, "xime");
        assert_eq!(meta.distribution_name, "Xime");
        assert_eq!(meta.distribution_code_name, "Xime");
        assert_eq!(meta.app_name, "rime.xime.setup");
        assert_eq!(meta.version, "1.0");
    }

    #[test]
    fn test_metadata_is_singleton() {
        let a = app_metadata() as *const _;
        let b = app_metadata() as *const _;
        assert_eq!(a, b);
    }
}
