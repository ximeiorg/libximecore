#![cfg(feature = "clipboard-page")]
use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{button_primary, label};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::row;
use iced::Element;

pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    settings_page(
        "剪贴板",
        colors,
        vec![
            settings_group(
                "剪贴板历史",
                Some("管理剪贴板历史记录"),
                colors,
                vec![settings_item(
                    "启用剪贴板历史",
                    Some("记录复制历史以便快速粘贴"),
                    colors,
                    label("开发中", colors),
                )],
            ),
            settings_group(
                "操作",
                None::<String>,
                colors,
                vec![row![button_primary("清空历史", colors, Message::ClearClipboardHistory)].into()],
            ),
        ],
    )
}
