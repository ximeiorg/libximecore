//! 语音转文本页。
//!
//! **产品结构**（一次只让用户看一个概念，别把两套引擎摊成两张表单）：
//!
//! 1. 顶部**状态大卡**：一眼看到「能不能用、用哪个模型、走 CPU 还是 GPU」，
//!    主操作只有一个（试听）；
//! 2. **语音模型**：下拉选择当前模型（当前用哪个是**单值配置**，不是一排
//!    「使用」按钮——见 DECISIONS「词典 = 下拉选择」那条），选中的那个模型
//!    才显示下载/删除与进度；
//! 3. **试听结果**：说出的话 + 复制；
//! 4. **系统听写**：Windows 自带在线听写，只在本页试听、不参与上屏，弱化处理；
//! 5. **说明**：候选栏 🎙️ 怎么用。
//!
//! 两条链路故意不合并：本地模型跑在 `winxime-server` 进程（与候选栏 🎙️ 共用
//! 一条会话），WinRT 跑在设置进程，各持状态机、各管自己的麦克风。
#![cfg(all(feature = "voice-page", windows))]

use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{
    badge, button_disabled, button_primary, button_secondary, card_style, pick_list_style,
    progress, semibold, text_button, UI_FONT,
};
use crate::speech_models::{SpeechModelEntry, SpeechStatusKind};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{column, container, pick_list, row, text, Space};
use iced::{border, Alignment, Background, Border, Color, Element, Length};

/// 顶部状态大卡里图标块的尺寸。
const HERO_GLYPH: f32 = 44.0;

pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let server = &settings.speech_server;

    settings_page(
        "语音转文本",
        colors,
        vec![
            hero(server, colors),
            model_card(server, colors),
            preview_card(server, colors),
            dictation_card(settings, colors),
            help_card(colors),
        ],
    )
}

/// 顶部状态大卡：图标 + 状态徽标 + 当前模型 + 主操作（试听）。
fn hero<'a>(
    server: &'a crate::speech_models::SpeechModelState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let kind = server.status_kind();
    let (badge_fg, badge_bg) = status_colors(kind, colors);

    let mut title_line = row![
        badge(kind.label(), badge_fg, badge_bg),
        badge(
            server.backend_label(),
            colors.foreground_muted,
            colors.surface_variant
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if server
        .selected_entry()
        .map(|e| e.recommended)
        .unwrap_or(false)
    {
        title_line = title_line.push(badge("推荐", colors.on_primary, colors.primary));
    }

    let head = column![
        title_line,
        text(server.model_name())
            .size(19)
            .font(semibold())
            .color(colors.foreground),
        text(kind.hint()).size(13).color(colors.foreground_muted),
    ]
    .spacing(6)
    .width(Length::Fill);

    let body = row![
        hero_glyph(kind, colors),
        head,
        // 主操作只有一个：试听（试听不上屏，上屏在候选栏）。
        button_primary(server.preview_label(), colors, Message::SpeechPreviewToggle),
    ]
    .spacing(14)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(body)
        .width(Length::Fill)
        .padding(18)
        .style(move |_| card_style(colors))
        .into()
}

/// 状态图标块：本地离线语音（🅰 不引 emoji 字体，用汉字块，与商店卡片同款）。
fn hero_glyph<'a>(kind: SpeechStatusKind, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    let (bg, fg) = if kind.is_problem() {
        (colors.error_dim, colors.error)
    } else if kind == SpeechStatusKind::Listening {
        (colors.primary_dim, colors.primary)
    } else {
        (colors.tertiary_dim, colors.tertiary)
    };
    container(text("音").size(20).font(semibold()).color(fg))
        .width(HERO_GLYPH)
        .height(HERO_GLYPH)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: border::radius(14.0),
            },
            ..container::Style::default()
        })
        .into()
}

/// 语音模型卡：下拉选模型（单值配置）+ 选中模型的操作与进度。
fn model_card<'a>(
    server: &'a crate::speech_models::SpeechModelState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let mut items: Vec<Element<'a, Message>> = Vec::new();

    // 服务没起来：给一句明确的话，而不是一个空下拉。
    let choices = server.choices();
    if choices.is_empty() {
        let msg = if server.offline {
            "输入法服务未运行，暂时读不到模型列表"
        } else {
            "正在读取模型列表…"
        };
        items.push(text(msg).size(14).color(colors.foreground_muted).into());
        return settings_group(
            "语音模型",
            Some("模型只下载一次，之后完全离线运行，语音不会上传"),
            colors,
            items,
        );
    }

    // 工具行：下拉 + 已下载/未下载徽标。
    let selector = pick_list(choices, server.current_choice(), |choice| {
        Message::SpeechModelSelect(choice.id)
    })
    .placeholder("选择模型")
    .text_size(14)
    .padding([6, 10])
    .width(Length::Fixed(360.0))
    .style(move |_theme, status| pick_list_style(colors, status));
    items.push(row![selector].spacing(8).align_y(Alignment::Center).into());

    if let Some(entry) = server.selected_entry() {
        items.push(selected_model_detail(entry, server, colors));
    } else if server.offline {
        items.push(
            text("服务启动后这里会显示当前模型的状态与下载入口")
                .size(13)
                .color(colors.foreground_faint)
                .into(),
        );
    }

    // 操作结果提示（下载/删除/切换的成败都在这一行，5 秒后淡出）。
    if let Some(message) = server.message() {
        items.push(
            text(message.to_string())
                .size(13)
                .color(colors.primary)
                .into(),
        );
    }

    settings_group(
        "语音模型",
        Some("换模型立刻生效，不用重启输入法；同一个模型只下载一次"),
        colors,
        items,
    )
}

/// 选中模型的详情块：状态徽标 + 描述 + 大小 + 操作（下载/重新下载/删除）+ 进度。
fn selected_model_detail<'a>(
    entry: &'a SpeechModelEntry,
    server: &'a crate::speech_models::SpeechModelState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let progress_value = server.download_progress(&entry.id);
    let downloading = progress_value.is_some();

    // 标题行：名字 + 状态徽标 + 大小。
    let (state_fg, state_bg) = if entry.downloaded {
        (colors.success, colors.success_dim)
    } else {
        (colors.foreground_muted, colors.surface_variant)
    };
    let mut title = row![
        text(entry.name.clone())
            .size(15)
            .font(semibold())
            .color(colors.foreground),
        badge(entry.state_label(), state_fg, state_bg),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if entry.recommended {
        title = title.push(badge("推荐", colors.on_primary, colors.primary));
    }
    title = title.push(container(text("")).width(Length::Fill));
    title = title.push(
        text(format!("下载包 {}", entry.size))
            .size(13)
            .color(colors.foreground_muted),
    );

    let mut block = column![title]
        .spacing(8)
        .width(Length::Fill)
        .padding([14, 4]);

    block = block.push(
        text(entry.description.clone())
            .size(13)
            .color(colors.foreground_muted),
    );
    // 工程 id 放最小的字：排错/对文档时有用，平时不抢视线。
    block = block.push(
        text(entry.id.clone())
            .size(12)
            .font(UI_FONT)
            .color(colors.foreground_faint),
    );

    if let Some(value) = progress_value {
        let percent = (value * 100.0).round().clamp(0.0, 100.0) as i32;
        block = block.push(
            row![
                progress(value, colors),
                text(format!("{percent}%")).size(13).color(colors.primary),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .width(Length::Fill),
        );
    }

    // 操作行：只作用于这一个（选中的）模型。
    let engine_busy = server.busy() && !downloading;
    let mut actions = row![].spacing(8).align_y(Alignment::Center);
    if downloading {
        let percent = (progress_value.unwrap_or(0.0) * 100.0).round() as i32;
        actions = actions.push(
            text(format!("正在下载… {percent}%（完成后自动可用）"))
                .size(13)
                .color(colors.foreground_muted),
        );
    } else if engine_busy {
        // 正在听写/装载：模型文件可能正被用着，这时改目录会让两边都不确定。
        actions = actions.push(button_disabled("正在使用中…", colors));
    } else if entry.downloaded {
        actions = actions.push(button_secondary(
            "重新下载",
            colors,
            Message::SpeechModelDownload(entry.id.clone()),
        ));
        actions = actions.push(text_button(
            "删除",
            colors,
            Message::SpeechModelDelete(entry.id.clone()),
        ));
    } else {
        actions = actions.push(button_primary(
            format!("下载（{}）", entry.size),
            colors,
            Message::SpeechModelDownload(entry.id.clone()),
        ));
        actions = actions.push(
            text("下载完成后即可开始听写")
                .size(13)
                .color(colors.foreground_faint),
        );
    }
    block.push(actions).into()
}

/// 试听结果卡：说出的话 + 复制。
fn preview_card<'a>(
    server: &'a crate::speech_models::SpeechModelState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let text_value = server.text();
    let body: Element<'a, Message> = if text_value.is_empty() {
        text("点上方「试听说一句」，识别结果会显示在这里")
            .size(13)
            .color(colors.foreground_muted)
            .into()
    } else {
        text(text_value).size(15).color(colors.foreground).into()
    };
    let panel = container(body)
        .width(Length::Fill)
        .padding([12, 14])
        .style(move |_| container::Style {
            background: Some(Background::Color(colors.surface_variant)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: border::radius(10.0),
            },
            ..container::Style::default()
        });

    let mut items: Vec<Element<'a, Message>> = vec![panel.into()];
    if !text_value.is_empty() {
        items.push(
            row![
                text_button("复制文本", colors, Message::SpeechModelCopy),
                text("试听结果只在本页显示，不会输入到别的窗口")
                    .size(13)
                    .color(colors.foreground_faint),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
        );
    }

    settings_group(
        "试听结果",
        Some("在这里先试试效果；真正上屏用打字时的候选栏 🎙️"),
        colors,
        items,
    )
}

/// 系统听写（WinRT，仅本页试听）——弱化处理，说明它不参与输入法。
fn dictation_card<'a>(
    settings: &'a SettingsState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let s = &settings.speech;
    let status: String = if s.listening {
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
        colors.error
    } else {
        colors.foreground_muted
    };
    let body: Element<'a, Message> = if s.text.is_empty() {
        text("（还没有识别内容）")
            .size(13)
            .color(colors.foreground_muted)
            .into()
    } else {
        text(&s.text).size(14).color(colors.foreground).into()
    };
    let panel = container(body)
        .width(Length::Fill)
        .padding([12, 14])
        .style(move |_| container::Style {
            background: Some(Background::Color(colors.surface_variant)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: border::radius(10.0),
            },
            ..container::Style::default()
        });

    let toggle = if s.listening || s.processing {
        "停止听写"
    } else {
        "开始听写"
    };

    settings_group(
        "系统听写（仅本页试听）",
        Some("Windows 自带的在线听写，用来在本页对比效果；它不参与输入法上屏"),
        colors,
        vec![
            settings_item(
                "系统听写状态",
                Some("由 Windows 语音识别提供，需要联网与麦克风权限"),
                colors,
                column![
                    text(status).size(13).color(status_color),
                    row![
                        button_secondary(toggle, colors, Message::SpeechToggle),
                        text_button("复制", colors, Message::SpeechCopy),
                        text_button("清空", colors, Message::SpeechClear),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                ]
                .spacing(8)
                .into(),
            ),
            settings_item("识别文本", None::<String>, colors, panel.into()),
        ],
    )
}

/// 说明卡：怎么用、文件在哪、GPU 说明。
fn help_card<'a>(colors: &'a ThemeColors) -> Element<'a, Message> {
    settings_group(
        "使用说明",
        None::<String>,
        colors,
        vec![
            settings_item(
                "怎么上屏",
                Some("打字时点候选栏的 🎙️ 开始说话，停顿时自动上屏；上屏失败会自动复制到剪贴板，按 Ctrl+V 粘贴"),
                colors,
                Space::new().width(Length::Shrink).into(),
            ),
            settings_item(
                "模型文件",
                Some("%APPDATA%\\Xime\\models\\<模型 id>\\：encoder / decoder / joiner / tokens 四个文件，删掉目录即等于未下载"),
                colors,
                Space::new().width(Length::Shrink).into(),
            ),
            settings_item(
                "GPU 加速",
                Some("安装 CUDA 版安装包即优先用显卡推理；机器上缺 CUDA 运行库时自动回退 CPU（日志里有提示），不会报错"),
                colors,
                Space::new().width(Length::Shrink).into(),
            ),
            settings_item(
                "隐私",
                Some("语音识别完全在本机进行，音频不出电脑；只有点「下载」时会联网取模型文件"),
                colors,
                Space::new().width(Length::Shrink).into(),
            ),
        ],
    )
}

/// 状态徽标配色。
fn status_colors(kind: SpeechStatusKind, colors: &ThemeColors) -> (Color, Color) {
    match kind {
        SpeechStatusKind::Ready => (colors.success, colors.success_dim),
        SpeechStatusKind::Listening => (colors.on_primary, colors.primary),
        SpeechStatusKind::Loading => (colors.tertiary, colors.tertiary_dim),
        SpeechStatusKind::ModelMissing => (colors.error, colors.error_dim),
        SpeechStatusKind::Offline => (colors.foreground_muted, colors.surface_variant),
        SpeechStatusKind::Connecting => (colors.foreground_muted, colors.surface_variant),
    }
}
