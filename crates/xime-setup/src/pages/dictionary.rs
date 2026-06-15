use crate::components::{SettingsControl, SettingsGroup, SettingsItem, SettingsPage};
use crate::state::SettingsState;
use crate::theme::ThemeColors;
use gpui::*;

pub fn render(
    _settings: &Entity<SettingsState>,
    colors: &ThemeColors,
) -> impl IntoElement {
    SettingsPage::new("词典管理", colors.clone())
        .group(
            SettingsGroup::new("用户词典", colors.clone())
                .description("管理用户词库")
                .items(vec![
                    SettingsItem::new("用户词典", SettingsControl::label("Rime 自动管理"))
                        .description("用户词典由 Rime 引擎自动维护"),
                ]),
        )
}
