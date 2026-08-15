#![cfg(feature = "pair-page")]
use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{button_primary, label};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::row;
use iced::Element;

pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    settings_page(
        "设备关联",
        colors,
        vec![
            settings_group(
                "设备配对",
                Some("通过配对码关联多台设备"),
                colors,
                vec![settings_item(
                    "配对状态",
                    Some("当前设备未关联到任何账户"),
                    colors,
                    label("未配对", colors),
                )],
            ),
            settings_group(
                "操作",
                None::<String>,
                colors,
                vec![row![button_primary("开始配对", colors, Message::StartPairing)].into()],
            ),
        ],
    )
}
