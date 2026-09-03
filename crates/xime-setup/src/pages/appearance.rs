use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{button_primary, number_input};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{pick_list, row};
use iced::Element;

/// Dark mode options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DarkModeOption {
    Light,
    Dark,
    FollowSystem,
}

impl DarkModeOption {
    pub fn label(&self) -> &str {
        match self {
            DarkModeOption::Light => "浅色",
            DarkModeOption::Dark => "深色",
            DarkModeOption::FollowSystem => "跟随系统",
        }
    }

    pub fn from_value(v: u8) -> Self {
        match v {
            0 => DarkModeOption::Light,
            1 => DarkModeOption::Dark,
            _ => DarkModeOption::FollowSystem,
        }
    }

    pub fn to_value(&self) -> u8 {
        match self {
            DarkModeOption::Light => 0,
            DarkModeOption::Dark => 1,
            DarkModeOption::FollowSystem => 2,
        }
    }
}

impl std::fmt::Display for DarkModeOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let appearance = &settings.appearance;

    // Get available color scheme names for pick lists
    let scheme_names: Vec<String> = appearance
        .available_color_schemes
        .iter()
        .map(|(id, _, _)| id.clone())
        .collect();

    // Get current light/dark scheme names
    let (light_scheme, dark_scheme) = match &appearance.color_scheme {
        xime_config::ColorSchemeConfig::Simple(name) => (name.clone(), name.clone()),
        xime_config::ColorSchemeConfig::Named { light, dark } => (light.clone(), dark.clone()),
    };

    let dark_mode_option = DarkModeOption::from_value(appearance.dark_mode.value());

    settings_page(
        "外观",
        colors,
        vec![
            settings_group(
                "主题",
                None::<String>,
                colors,
                vec![
                    settings_item(
                        "外观模式",
                        Some("浅色 / 深色 / 跟随系统"),
                        colors,
                        pick_list(
                            vec![
                                DarkModeOption::Light,
                                DarkModeOption::Dark,
                                DarkModeOption::FollowSystem,
                            ],
                            Some(dark_mode_option),
                            |mode| Message::DarkModeChanged(mode.to_value()),
                        )
                        .into(),
                    ),
                    settings_item(
                        "浅色配色",
                        Some("浅色模式下使用的配色方案"),
                        colors,
                        pick_list(
                            scheme_names.clone(),
                            Some(light_scheme),
                            Message::ColorSchemeLightChanged,
                        )
                        .into(),
                    ),
                    settings_item(
                        "深色配色",
                        Some("深色模式下使用的配色方案"),
                        colors,
                        pick_list(
                            scheme_names,
                            Some(dark_scheme),
                            Message::ColorSchemeDarkChanged,
                        )
                        .into(),
                    ),
                ],
            ),
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
