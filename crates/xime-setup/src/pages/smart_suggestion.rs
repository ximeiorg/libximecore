#![cfg(feature = "smart-suggestion-page")]
use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{button_primary, label};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::row;
use iced::Element;

pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    settings_page(
        "智能联想",
        colors,
        vec![
            settings_group(
                "AI 智能联想",
                Some("基于 ONNX 模型的智能输入联想"),
                colors,
                vec![
                    settings_item(
                        "启用智能联想",
                        Some("开启后输入时自动联想下一个词"),
                        colors,
                        label("开发中", colors),
                    ),
                    settings_item(
                        "联想数量",
                        Some("每次显示的建议数量"),
                        colors,
                        label("5", colors),
                    ),
                ],
            ),
            settings_group(
                "操作",
                None::<String>,
                colors,
                vec![row![button_primary(
                    "保存设置",
                    colors,
                    Message::SaveSmartSuggestion
                )]
                .into()],
            ),
        ],
    )
}
