use crate::components::{SettingsControl, SettingsGroup, SettingsItem, SettingsPage};
use crate::state::SettingsState;
use crate::theme::ThemeColors;
use gpui::*;

pub fn render(settings: &Entity<SettingsState>, colors: &ThemeColors) -> impl IntoElement {
    let page = SettingsPage::new("输入方案", colors.clone());
    page
}
