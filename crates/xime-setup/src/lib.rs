pub mod app;
pub mod components;
pub mod pages;
pub mod state;
pub mod theme;

pub use app::{run, SettingsApp};
pub use state::SettingsState;
pub use state::{
    set_notify_deploy, set_notify_message, set_notify_reload_plugins, set_notify_reload_style,
    set_notify_select_schema,
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
