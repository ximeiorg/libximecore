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
