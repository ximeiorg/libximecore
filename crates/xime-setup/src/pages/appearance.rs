use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{button_primary, number_input};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::row;
use iced::Element;

pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let appearance = &settings.appearance;

    settings_page(
        "外观",
        colors,
        vec![
            settings_group(
                "显示",
                None::<String>,
                colors,
                vec![
                    settings_item(
                        "字号",
                        Some("候选词显示字号"),
                        colors,
                        number_input(
                            appearance.font_size,
                            8.0,
                            48.0,
                            1.0,
                            colors,
                            Message::FontSizeChanged,
                        ),
                    ),
                    settings_item(
                        "候选词数量",
                        Some("候选词列表中显示的数量"),
                        colors,
                        number_input(
                            appearance.candidate_count as f64,
                            1.0,
                            20.0,
                            1.0,
                            colors,
                            |v| Message::CandidateCountChanged(v as i32),
                        ),
                    ),
                    settings_item(
                        "圆角大小",
                        Some("候选窗口圆角半径"),
                        colors,
                        number_input(
                            appearance.corner_radius,
                            0.0,
                            24.0,
                            1.0,
                            colors,
                            Message::CornerRadiusChanged,
                        ),
                    ),
                ],
            ),
            settings_group(
                "操作",
                None::<String>,
                colors,
                vec![row![button_primary(
                    "保存外观设置",
                    colors,
                    Message::SaveAppearance
                )]
                .into()],
            ),
        ],
    )
}
