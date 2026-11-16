use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{
    button_danger, button_disabled, button_primary, button_secondary, medium, number_input,
    pick_list_style, switch,
};
use crate::state::{AppearanceState, Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{button, column, container, pick_list, row, text};
use iced::{border, Background, Border, Color, Element, Length};

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

/// 预览里用的示例候选词（第 1 个画成选中态）。
#[rustfmt::skip]
const PREVIEW_WORDS: [&str; 10] = ["曦码", "输入", "输入法", "五笔", "字根", "候选", "词频", "重码", "上屏", "词组"];

/// 候选栏预览用到的颜色（取自当前配色的 `keyboard_background` 与候选色字段）。
struct PreviewPalette {
    bar_bg: Color,
    text: Color,
    selected_bg: Color,
    selected_text: Color,
}

/// 32 位色 → `Color`（只取低 24 位）。
fn color_of(value: u32) -> Color {
    Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// 32 位色 → `Color`，`0xAARRGGBB` 时带上透明度。
fn color_with_alpha(value: u32) -> Color {
    if value > 0x00FF_FFFF {
        Color::from_rgba8(
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
            (((value >> 24) & 0xFF) as f32) / 255.0,
        )
    } else {
        color_of(value)
    }
}

/// 底色上可读的前景色（按相对亮度取深/浅）。
fn readable_on(bg: Color) -> Color {
    let luma = 0.2126 * bg.r + 0.7152 * bg.g + 0.0722 * bg.b;
    if luma > 0.5 {
        Color::from_rgb8(0x1a, 0x1a, 0x1e)
    } else {
        Color::WHITE
    }
}

fn preview_palette(appearance: &AppearanceState, app_is_dark: bool) -> PreviewPalette {
    let dark = appearance.dark_mode.is_dark(app_is_dark);
    let scheme = appearance.color_schemes.get(&appearance.color_scheme);

    let bar_bg = scheme
        .and_then(|s| s.keyboard_bg(dark))
        .map(color_of)
        .unwrap_or(if dark {
            Color::from_rgb8(0x1E, 0x18, 0x38)
        } else {
            Color::from_rgb8(0xE3, 0xE4, 0xE8)
        });
    let text = scheme
        .and_then(|s| s.candidate_text(dark))
        .map(color_of)
        .unwrap_or_else(|| readable_on(bar_bg));
    // 选中底色未配置时按主色铺满（与候选窗一致）。
    let selected_bg = scheme
        .and_then(|s| s.candidate_selected_bg(dark))
        .map(color_with_alpha)
        .unwrap_or_else(|| color_of(scheme.map(|s| s.primary_color).unwrap_or(0x8F73E2)));
    let selected_text = scheme
        .and_then(|s| s.candidate_selected_text(dark))
        .map(color_of)
        .unwrap_or_else(|| readable_on(selected_bg));

    PreviewPalette {
        bar_bg,
        text,
        selected_bg,
        selected_text,
    }
}

/// 顶部预览卡片：按当前「字号 / 候选数量 / 圆角 / 横竖排 / 配色」画一条候选栏。
fn preview_card<'a>(
    appearance: &'a AppearanceState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let colors = *colors;
    let palette = preview_palette(appearance, colors.is_dark);
    // 配置里的字号是 pt，iced 用 px（1pt ≈ 4/3px）。
    let font = (appearance.font_size * 4.0 / 3.0).clamp(11.0, 40.0) as f32;
    let radius = appearance.corner_radius.clamp(0.0, 24.0) as f32;
    let item_radius = (radius - 3.0).max(2.0);
    let count = appearance.candidate_count.clamp(1, 10) as usize;

    let items: Vec<Element<'a, Message>> = (0..count)
        .map(|i| {
            let selected = i == 0;
            let word = PREVIEW_WORDS[i % PREVIEW_WORDS.len()];
            let fg = if selected {
                palette.selected_text
            } else {
                palette.text
            };
            let num_color = fg.scale_alpha(0.7);
            let bg = palette.selected_bg;
            container(
                row![
                    text(format!("{}.", i + 1))
                        .size(font * 0.85)
                        .color(num_color),
                    text(word).size(font).color(fg),
                ]
                .spacing(3.0),
            )
            .padding([2.0, 6.0])
            .style(move |_| container::Style {
                background: if selected {
                    Some(Background::Color(bg))
                } else {
                    None
                },
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: border::radius(item_radius),
                },
                ..container::Style::default()
            })
            .into()
        })
        .collect();

    let bar_inner: Element<'a, Message> = if appearance.horizontal {
        // 候选多了就换行，宁可折行也不要横向溢出卡片。
        row(items).spacing(2.0).wrap().into()
    } else {
        column(items).spacing(2.0).into()
    };

    let bar_bg = palette.bar_bg;
    let bar_text = palette.text;
    let bar = container(bar_inner)
        .width(Length::Fill)
        .padding([6.0, 8.0])
        .style(move |_| container::Style {
            background: Some(Background::Color(bar_bg)),
            text_color: Some(bar_text),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: border::radius(radius),
            },
            ..container::Style::default()
        });

    // 外壳留一圈页面底色，让候选栏像浮在桌面上。
    let shell = container(bar)
        .width(Length::Fill)
        .padding(14)
        .style(move |_| container::Style {
            background: Some(Background::Color(colors.surface_variant)),
            border: Border {
                color: colors.border,
                width: 1.0,
                radius: border::radius(10.0),
            },
            ..container::Style::default()
        });

    settings_group(
        "预览",
        Some("候选栏样式（随下方设置即时变化）"),
        &colors,
        vec![shell.into()],
    )
}

/// 一个配色方案在色卡墙上需要的数据。
struct SchemeEntry {
    id: String,
    name: String,
    primary: u32,
}

/// 配色方案列表：`name` 当显示名用（id 只做键），按名字排序——`HashMap` 的
/// 迭代顺序每次都不一样，直接铺色卡会跳位。
fn scheme_entries(appearance: &AppearanceState) -> Vec<SchemeEntry> {
    let mut list: Vec<SchemeEntry> = if appearance.color_schemes.is_empty() {
        appearance
            .available_color_schemes
            .iter()
            .map(|(id, name, primary)| SchemeEntry {
                id: id.clone(),
                name: name.clone(),
                primary: *primary,
            })
            .collect()
    } else {
        appearance
            .color_schemes
            .iter()
            .map(|(id, scheme)| SchemeEntry {
                id: id.clone(),
                name: scheme.name.clone(),
                primary: scheme.primary_color,
            })
            .collect()
    };
    list.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    list
}

/// 一张色卡：底色/文字色直接取方案自身的值，左侧一个主色圆点，选中时描边加粗。
fn scheme_chip<'a>(
    appearance: &'a AppearanceState,
    entry: SchemeEntry,
    dark: bool,
    current: &str,
    colors: &'a ThemeColors,
    on_pick: fn(String) -> Message,
) -> Element<'a, Message> {
    let scheme = appearance.color_schemes.get(&entry.id);
    let bg = scheme
        .and_then(|s| s.keyboard_bg(dark))
        .map(color_of)
        .unwrap_or(colors.surface_variant);
    let fg = scheme
        .and_then(|s| s.candidate_text(dark))
        .map(color_of)
        .unwrap_or_else(|| readable_on(bg));
    let accent = color_of(entry.primary);
    let selected = entry.id == current;
    let idle_border = colors.border;

    let dot = container(text("").size(1.0))
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(accent)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: border::radius(4.0),
            },
            ..container::Style::default()
        });

    button(
        row![dot, text(entry.name).size(12).color(fg)]
            .spacing(6)
            .align_y(iced::Alignment::Center),
    )
    .padding([6, 10])
    .style(move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: if selected {
                    accent
                } else if hovered {
                    accent.scale_alpha(0.55)
                } else {
                    idle_border
                },
                width: if selected { 2.0 } else { 1.0 },
                radius: border::radius(9.0),
            },
            ..button::Style::default()
        }
    })
    .on_press(on_pick(entry.id))
    .into()
}

/// 一段配色区块：标题 + 说明 + 可换行的色卡墙。
///
/// 一张卡 = 一套方案（自带浅色与深色变体），所以只有一道色卡墙、一个选中项；
/// 卡片按当前生效的明暗渲染，切换「外观模式」就能对比同一方案的两种样子。
fn scheme_section<'a>(
    title: &'a str,
    hint: &'a str,
    appearance: &'a AppearanceState,
    colors: &'a ThemeColors,
    on_pick: fn(String) -> Message,
) -> Element<'a, Message> {
    let dark = appearance.dark_mode.is_dark(colors.is_dark);
    let chips: Vec<Element<'a, Message>> = scheme_entries(appearance)
        .into_iter()
        .map(|entry| {
            scheme_chip(
                appearance,
                entry,
                dark,
                &appearance.color_scheme,
                colors,
                on_pick,
            )
        })
        .collect();

    column![
        text(title).size(14).font(medium()).color(colors.foreground),
        text(hint)
            .size(13)
            .font(medium())
            .color(colors.foreground_muted),
        row(chips).spacing(8).wrap(),
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let appearance = &settings.appearance;

    let dark_mode_option = DarkModeOption::from_value(appearance.dark_mode.value());

    settings_page(
        "外观",
        colors,
        vec![
            preview_card(appearance, colors),
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
                        ).text_size(14).padding([8, 10]).width(180.0)
                        .style(move |_theme, status| pick_list_style(colors, status))
                        .into(),
                    ),
                    scheme_section(
                        "配色方案",
                        "每个方案自带浅色 / 深色两套颜色，切换上方「外观模式」即可对比",
                        appearance,
                        colors,
                        Message::ColorSchemeChanged,
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
                        Some("候选词列表中显示的数量（1–10，第 10 个用 0 选）"),
                        colors,
                        number_input(
                            appearance.candidate_count as f64,
                            1.0,
                            10.0,
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
                    settings_item(
                        "候选栏横向排列",
                        Some("开：候选横排；关：候选竖排（每行一个）"),
                        colors,
                        switch(appearance.horizontal, colors, Message::HorizontalChanged),
                    ),
                    settings_item(
                        "数学计算",
                        Some("数字开头自动识别为算式，按 = 或工具栏计算"),
                        colors,
                        switch(appearance.auto_detect, colors, Message::AutoDetectChanged),
                    ),
                ],
            ),
            settings_group(
                "操作",
                None::<String>,
                colors,
                vec![
                    row![button_primary(
                        "保存外观设置",
                        colors,
                        Message::SaveAppearance
                    )]
                    .into(),
                    // 配置文件本身的操作：撤销上一次修改 / 恢复默认（删掉用户文件）/
                    // 恢复为用户最初提供的那份（`.orig`，仅在程序改写前存在过时可用）。
                    {
                        let file_state = SettingsState::config_file_state();
                        let undo = if file_state.backup {
                            button_secondary("撤销上一次修改", colors, Message::UndoConfigChange)
                                .into()
                        } else {
                            button_disabled("撤销上一次修改", colors)
                        };
                        let restore_original = if file_state.original {
                            button_secondary(
                                "恢复为用户提供的配置",
                                colors,
                                Message::RestoreConfigOriginal,
                            )
                            .into()
                        } else {
                            button_disabled("恢复为用户提供的配置", colors)
                        };
                        let restore_defaults = if file_state.user_file {
                            button_danger("恢复默认配置", colors, Message::RestoreConfigDefaults)
                                .into()
                        } else {
                            button_disabled("恢复默认配置", colors)
                        };
                        row![undo, restore_original, restore_defaults].spacing(8).into()
                    },
                ],
            ),
        ],
    )
}
