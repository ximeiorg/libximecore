pub mod app;
pub mod backup;
pub mod components;
pub mod pages;
pub mod profile;
pub mod state;
pub mod theme;
pub mod webdav;

#[cfg(feature = "voice-page")]
pub mod speech;

/// 本地语音模型（server 侧引擎）在设置端的镜像与回调槽。
#[cfg(feature = "voice-page")]
pub mod speech_models;

/// 语音输入模式（本地 / 在线插件）与在线插件的配置表单。
#[cfg(feature = "voice-page")]
pub mod speech_mode;

pub use app::{run, run_with, SettingsApp};
#[cfg(feature = "voice-page")]
#[cfg(feature = "voice-page")]
pub use speech_mode::{
    read_config as read_speech_config, write_config as write_speech_config, CommitMode,
    SpeechConfig, SpeechMode, SpeechPluginInfo, SpeechUiState, LANGUAGES as SPEECH_LANGUAGES,
};
#[cfg(feature = "voice-page")]
pub use speech_models::{
    local_engine_registered, set_notify_speech_delete, set_notify_speech_download, set_notify_speech_import,
    set_notify_speech_select, set_notify_speech_status, set_notify_speech_test_start,
    set_notify_speech_test_stop, SpeechModelEntry, SpeechModelState, SpeechServerStatus,
};
pub use state::SettingsState;
pub use state::{
    set_notify_deploy, set_notify_deploy_toast, set_notify_message, set_notify_reload_plugins,
    set_notify_current_schema, set_notify_reload_style, set_notify_select_schema,
    set_notify_sync_user_data,
};
#[cfg(any(windows, feature = "dict-page"))]
pub use state::{
    set_notify_dict_backup, set_notify_dict_entries, set_notify_dict_entry_write,
    set_notify_dict_export, set_notify_dict_import, set_notify_dict_list, set_notify_dict_restore,
    set_notify_phrase_list, set_notify_phrase_save, set_notify_schema_entries,
};
pub use theme::{SystemTheme, ThemeColors};
pub use xime_config::{
    default_rime_paths, ensure_bundled_rime_data, set_app_metadata, set_rime_paths, AppMetadata,
    RimePaths,
};

use std::sync::OnceLock;

use rust_embed::RustEmbed;

/// 本库自带的通用图标（侧栏导航等），与具体产品无关。
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/assets"]
#[include = "icons/*.svg"]
pub struct Assets;

/// 设置界面的品牌素材：窗口图标 + 侧栏/关于页 logo。
///
/// 品牌属于产品仓库（如 XimeChe 的 `resources/`），本库不自带任何品牌图，
/// 由宿主在启动时通过 [`set_brand_assets`] 注入；未注入时界面不画 logo。
#[derive(Clone, Copy)]
pub struct BrandAssets {
    /// 窗口图标（PNG 字节）。
    pub app_icon_png: &'static [u8],
    /// 侧栏品牌区与关于页用的 logo（SVG 字节）。
    pub logo_svg: &'static [u8],
}

static BRAND_ASSETS: OnceLock<BrandAssets> = OnceLock::new();

/// 注入品牌素材。重复调用保留第一份（与 `set_app_metadata` 同语义）。
pub fn set_brand_assets(assets: BrandAssets) {
    let _ = BRAND_ASSETS.set(assets);
}

/// 已注入的品牌素材；宿主没注入时为 `None`。
pub fn brand_assets() -> Option<&'static BrandAssets> {
    BRAND_ASSETS.get()
}
