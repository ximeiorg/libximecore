use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::label;
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::Element;

pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    settings_page(
        "词典管理",
        colors,
        vec![settings_group(
            "用户词典",
            Some("管理用户词库"),
            colors,
            vec![settings_item(
                "用户词典",
                Some("用户词典由 Rime 引擎自动维护"),
                colors,
                label("Rime 自动管理", colors),
            )],
        )],
    )
}
