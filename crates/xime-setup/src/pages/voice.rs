//! 语音转文本页（Windows WinRT 听写；状态机对齐 Android 语音输入）。
#![cfg(all(feature = "voice-page", windows))]

use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{button_primary, label, text_button};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{container, row, text};
use iced::{Background, Border, Color, Element, Length};

pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let s = &settings.speech;

    // 引擎状态行。
    let status_text: String = if s.listening {
        "正在听写……请对着麦克风说话".to_string()
    } else if s.processing {
        "正在启动语音引擎…".to_string()
    } else if let Some(e) = &s.error {
        e.clone()
    } else {
        "就绪".to_string()
    };
    let status_color = if s.listening {
        colors.primary
    } else if s.error.is_some() {
        Color::from_rgb8(0xE5, 0x48, 0x3B)
    } else {
        colors.foreground_muted
    };

    // 识别文本展示区。
    let body: Element<'a, Message> = if s.text.is_empty() {
        text("（还没有识别内容，点上方按钮开始说话）")
            .size(13)
            .color(colors.foreground_muted)
            .into()
    } else {
        text(&s.text).size(14).color(colors.foreground).into()
    };
    let text_panel = container(body)
        .width(Length::Fill)
        .padding([10, 12])
        .style(move |_| container::Style {
            background: Some(Background::Color(Color {
                a: 0.06,
                ..colors.foreground
            })),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: iced::border::radius(8.0),
            },
            ..container::Style::default()
        });

    // 操作：开始/停止 + 文本操作。
    let toggle_label = if s.listening || s.processing {
        "停止听写"
    } else {
        "开始听写"
    };

    settings_page(
        "语音转文本",
        colors,
        vec![
            settings_group(
                "语音转文本",
                Some("对着麦克风说话，自动转成文字；使用 Windows 自带语音识别（在线听写），首次使用需允许访问麦克风并安装中文语音"),
                colors,
                vec![
                    settings_item(
                        "引擎状态",
                        None::<String>,
                        colors,
                        text(status_text).size(13).color(status_color).into(),
                    ),
                    settings_item(
                        "操作",
                        Some("再次点击按钮即可停止并保留已识别文本"),
                        colors,
                        button_primary(toggle_label, colors, Message::SpeechToggle).into(),
                    ),
                ],
            ),
            settings_group(
                "识别结果",
                Some("逐短语追加；可复制到剪贴板后粘贴到任意输入框"),
                colors,
                vec![
                    settings_item("文本", None::<String>, colors, text_panel.into()),
                    settings_item(
                        "文本操作",
                        None::<String>,
                        colors,
                        row![
                            text_button("复制文本", colors, Message::SpeechCopy),
                            text_button("清空", colors, Message::SpeechClear),
                        ]
                        .spacing(8)
                        .into(),
                    ),
                ],
            ),
            settings_group(
                "说明",
                None::<String>,
                colors,
                vec![settings_item(
                    "离线模型",
                    Some("当前为系统级听写（联网）。本地离线模型（与 Android 端同款 zipformer）在后续版本提供"),
                    colors,
                    label("", colors),
                )],
            ),
        ],
    )
}
