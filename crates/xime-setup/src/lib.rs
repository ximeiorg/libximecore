pub mod app;
pub mod backup;
pub mod components;
pub mod pages;
pub mod state;
pub mod theme;
pub mod webdav;

#[cfg(all(feature = "voice-page", windows))]
pub mod speech;

pub use app::{run, SettingsApp};
pub use state::SettingsState;
pub use state::{
    set_notify_deploy, set_notify_dict_backup, set_notify_dict_export, set_notify_dict_import,
    set_notify_dict_list, set_notify_dict_restore, set_notify_message, set_notify_reload_plugins,
    set_notify_reload_style, set_notify_select_schema, set_notify_sync_user_data,
};
pub use theme::{SystemTheme, ThemeColors};
pub use xime_config::{
    default_rime_paths, set_app_metadata, set_rime_paths, AppMetadata, RimePaths,
};

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/assets"]
#[include = "icons/*.svg"]
#[include = "image/*.png"]
pub struct Assets;
