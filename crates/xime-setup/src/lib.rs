pub mod components;
pub mod pages;
pub mod state;
pub mod theme;

#[cfg(target_os = "linux")]
pub mod webdav;

pub use pages::SettingsApp;
pub use state::SettingsState;
pub use state::{set_notify_deploy, set_notify_reload_style};
pub use theme::{SystemTheme, ThemeColors};

use gpui::*;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/assets"]
#[include = "icons/*.svg"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }
        Ok(<Assets as rust_embed::Embed>::get(path).map(|x| x.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(<Assets as rust_embed::Embed>::iter()
            .filter_map(|p| {
                if p.starts_with(path) {
                    Some(p.into())
                } else {
                    None
                }
            })
            .collect())
    }
}
