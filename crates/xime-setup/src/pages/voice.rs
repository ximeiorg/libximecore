//! 语音输入页。
//!
//! **产品结构**（一次只让用户看一个概念）：
//!
//! 1. 顶部**输入方式**：本地模型 / 在线模型二选一（同一时刻只有一个引擎在跑）；
//! 2. **状态大卡**：当前引擎能不能用、走哪个模型/插件；
//! 3. 本地：模型下拉 + 下载/导入/删除 + 试听；
//!    在线：插件下拉 + 按插件 `settings.schema` 渲染的配置表单 + 测试连接；
//! 4. **通用配置**：语种、上屏方式（两种模式共用，写进 `speech.toml`）；
//! 5. **说明**：候选栏 🎙️ 怎么用、文件在哪、隐私边界。
//!
//! 两条链路故意不合并：本地模型跑在输入法服务进程（与候选栏 🎙️ 共用一条会话），
//! 在线插件由输入法进程在识别时调用，配置由插件自己消费。设置端都只写配置。

use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{
    badge, button_disabled, button_primary, button_secondary, card_style, pick_list_style,
    progress, semibold, switch, tab_bar, text_button, text_input_style, UI_FONT,
};
use crate::speech_mode::{language_label, CommitMode, SpeechMode, SpeechUiState, LANGUAGES};
use crate::speech_models::{SpeechModelEntry, SpeechStatusKind};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{column, container, pick_list, row, text, text_input, Space};
use iced::{border, Alignment, Background, Border, Color, Element, Length};

/// 顶部状态大卡里图标块的尺寸。
const HERO_GLYPH: f32 = 44.0;

pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let server = &settings.speech_server;
    let ui = &settings.speech_mode;

    // 总开关是页面的第一道门：关着时只留开关与一句说明。已经不录音了，
    // 让用户继续在这里配模型/插件只会误导（他以为还有东西在跑）。
    let mut groups: Vec<Element<'a, Message>> = vec![enable_card(ui, colors)];
    if !ui.enabled {
        groups.push(off_card(colors));
        return settings_page("语音输入", colors, groups);
    }

    groups.push(mode_card(ui, colors));
    groups.push(hero(ui, server, colors));

    match ui.mode {
        SpeechMode::Local => {
            groups.push(model_card(server, colors));
            groups.push(preview_card(server, colors));
            // 系统听写卡是 Windows WinRT 专属（Linux 无此在线服务）。
            #[cfg(windows)]
            groups.push(dictation_card(settings, colors));
        }
        SpeechMode::Online => groups.push(online_card(ui, colors)),
    }

    groups.push(common_card(ui, colors));
    groups.push(help_card(ui.mode, colors));

    settings_page("语音输入", colors, groups)
}

/// 总开关：一键开启 / 关闭语音输入。
///
/// 开关值写进 `speech.toml` 的 `enabled`，输入法按 mtime 热加载——关闭后
/// 按住 Fn/Option、候选栏 🎙️、菜单栏「语音输入」一起失效，也不会再拉起
/// 语音服务进程。这是**唯一**的开关入口（菜单只反映状态，不提供切换）。
fn enable_card<'a>(ui: &'a SpeechUiState, colors: &'a ThemeColors) -> Element<'a, Message> {
    settings_group(
        "语音输入",
        Some("关闭后所有入口一起失效：快捷键、候选栏 🎙️、菜单栏；也不再启动语音服务"),
        colors,
        vec![settings_item(
            "启用语音输入",
            Some(if ui.enabled {
                "按住 Fn / Option 说话，说完松手即可上屏"
            } else {
                "当前已关闭：不会录音，也不会启动语音服务"
            }),
            colors,
            switch(ui.enabled, colors, Message::SpeechEnabledChanged),
        )],
    )
}

/// 关闭态的说明卡：把「关了什么」和「怎么再打开」写清楚，不留悬空页。
fn off_card<'a>(colors: &'a ThemeColors) -> Element<'a, Message> {
    let rows: Vec<Element<'a, Message>> = [
        ("快捷键", "按住 Fn / 左 Option 不再开始录音"),
        ("候选栏", "面板里的「语音输入」入口不再出现"),
        ("菜单栏", "输入法菜单里的「语音输入」不再出现"),
        ("语音服务", "不再按需拉起 xime-speechd，不占内存"),
    ]
    .into_iter()
    .map(|(name, desc)| {
        row![
            text(name)
                .size(13)
                .font(semibold())
                .color(colors.foreground)
                .width(Length::Fixed(72.0)),
            text(desc).size(13).color(colors.foreground_muted),
        ]
        .spacing(8)
        .into()
    })
    .collect();

    settings_group(
        "当前状态",
        Some("重新打开上面的开关即可恢复，模型与插件配置都还在"),
        colors,
        rows,
    )
}

/// 输入方式卡：本地 / 在线二选一。
fn mode_card<'a>(ui: &'a SpeechUiState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let bar = tab_bar(&SpeechMode::LABELS, ui.mode.index(), colors, |index| {
        Message::SpeechModeChanged(SpeechMode::from_index(index))
    });

    let mut items: Vec<Element<'a, Message>> = vec![
        row![bar].into(),
        text(ui.mode.hint())
            .size(13)
            .color(colors.foreground_muted)
            .into(),
    ];
    // 在线模式但一个语音插件都没装：这里直接把出路写清楚。
    if ui.mode == SpeechMode::Online && !ui.online_available() {
        items.push(
            text("还没有安装语音插件。到「扩展商店」安装一个 type 为 speech 的插件后回到这里。")
                .size(13)
                .color(colors.error)
                .into(),
        );
    }

    settings_group(
        "输入方式",
        Some("两种引擎只能二选一：本地模型离线跑，在线模型由插件调云端 API"),
        colors,
        items,
    )
}

/// 状态大卡：本地看服务/模型，在线看插件。
fn hero<'a>(
    ui: &'a SpeechUiState,
    server: &'a crate::speech_models::SpeechModelState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    match ui.mode {
        SpeechMode::Local => hero_local(server, colors),
        SpeechMode::Online => hero_online(ui, colors),
    }
}

/// 本地模型的状态大卡。
fn hero_local<'a>(
    server: &'a crate::speech_models::SpeechModelState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    // 当前平台没接入本地引擎（没有宿主注册状态回调）：别把「未接入」说成
    // 「服务未运行」，否则用户会去查一个根本不存在的服务。
    if !crate::speech_models::local_engine_registered() {
        let head = column![
            row![
                badge("未接入", colors.foreground_muted, colors.surface_variant),
                badge("本地模型", colors.foreground_muted, colors.surface_variant),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            text("当前平台还没有接入本地语音引擎")
                .size(19)
                .font(semibold())
                .color(colors.foreground),
            text("在线模型现在就能用；本地离线识别要等输入法进程接入引擎后开放")
                .size(13)
                .color(colors.foreground_muted),
        ]
        .spacing(6)
        .width(Length::Fill);
        let body = row![hero_glyph(true, false, colors), head]
            .spacing(14)
            .align_y(Alignment::Center)
            .width(Length::Fill);
        return container(body)
            .width(Length::Fill)
            .padding(18)
            .style(move |_| card_style(colors))
            .into();
    }

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
        hero_glyph(
            kind.is_problem(),
            kind == SpeechStatusKind::Listening,
            colors
        ),
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

/// 在线插件的状态大卡。
fn hero_online<'a>(ui: &'a SpeechUiState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let plugin = ui.selected_plugin();
    let (badge_fg, badge_bg, status_text): (Color, Color, &'static str) = match plugin {
        None => (
            colors.foreground_muted,
            colors.surface_variant,
            "未安装插件",
        ),
        Some(p) if p.enabled => (colors.success, colors.success_dim, "已启用"),
        Some(_) => (colors.error, colors.error_dim, "插件已停用"),
    };

    let title = plugin
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "在线模型".to_string());
    let hint = if plugin.is_some() {
        "语音会上传到插件配置的云端服务；配置项在下方，保存后即时生效"
    } else {
        "先到扩展商店安装一个语音插件，这里会按插件表单渲染配置项"
    };

    let head = column![
        row![
            badge("在线", colors.tertiary, colors.tertiary_dim),
            badge(status_text, badge_fg, badge_bg),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        text(title)
            .size(19)
            .font(semibold())
            .color(colors.foreground),
        text(hint).size(13).color(colors.foreground_muted),
    ]
    .spacing(6)
    .width(Length::Fill);

    let action: Element<'a, Message> = if ui.online_available() {
        button_secondary("测试连接", colors, Message::SpeechPluginTest).into()
    } else {
        store_button(colors)
    };

    let body = row![hero_glyph(true, false, colors), head, action,]
        .spacing(14)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    container(body)
        .width(Length::Fill)
        .padding(18)
        .style(move |_| card_style(colors))
        .into()
}

/// 状态图标块：本地离线语音（不引 emoji 字体，用汉字块，与商店卡片同款）。
fn hero_glyph<'a>(problem: bool, listening: bool, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    let (bg, fg) = if problem {
        (colors.error_dim, colors.error)
    } else if listening {
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

/// 「去扩展商店」按钮（商店页没编进侧栏时退化成一句提示）。
fn store_button<'a>(colors: &'a ThemeColors) -> Element<'a, Message> {
    match crate::pages::store_page_index() {
        Some(index) => button_primary("去扩展商店", colors, Message::PageSelected(index)).into(),
        None => button_disabled("请从扩展商店安装插件", colors),
    }
}

/// 本地语音模型卡：下拉选模型（单值配置）+ 选中模型的操作与进度 + 导入。
fn model_card<'a>(
    server: &'a crate::speech_models::SpeechModelState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let mut items: Vec<Element<'a, Message>> = Vec::new();

    // 服务没起来：给一句明确的话，而不是一个空下拉。
    let choices = server.choices();
    if choices.is_empty() {
        let msg = if !crate::speech_models::local_engine_registered() {
            "当前平台尚未接入本地语音引擎，暂时没有可管理的模型"
        } else if server.offline {
            "输入法服务未运行，暂时读不到模型列表"
        } else {
            "正在读取模型列表…"
        };
        items.push(text(msg).size(14).color(colors.foreground_muted).into());
        items.push(import_row(colors));
        return settings_group(
            "本地模型",
            Some("模型只下载一次，之后完全离线运行，语音不会上传"),
            colors,
            items,
        );
    }

    // 工具行：下拉 + 导入（商店里没有的模型可以由用户自己导入）。
    let selector = pick_list(choices, server.current_choice(), |choice| {
        Message::SpeechModelSelect(choice.id)
    })
    .placeholder("选择模型")
    .text_size(14)
    .padding([8, 10])
    .width(Length::Fixed(360.0))
    .style(move |_theme, status| pick_list_style(colors, status))
    .menu_style(crate::components::widgets::pick_list_menu_style(colors));
    items.push(
        row![selector, import_row(colors)]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
    );

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
        "本地模型",
        Some("换模型立刻生效，不用重启输入法；导入的模型与商店下载的模型等价"),
        colors,
        items,
    )
}

/// 「导入模型」：权重来源不限于扩展商店（离线拷贝、自训练模型都走这里）。
fn import_row<'a>(colors: &'a ThemeColors) -> Element<'a, Message> {
    row![
        text_button("导入模型…", colors, Message::SpeechModelImport),
        text("支持含 model.safetensors 的模型目录")
            .size(12)
            .color(colors.foreground_faint),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
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

/// 试听结果卡：说出的话 + 复制（本地模型专用；在线插件在候选栏里用）。
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
    let panel = panel(body, colors);

    let mut items: Vec<Element<'a, Message>> = vec![panel];
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

/// 在线模型卡：插件选择 + 插件表单 + 保存提示。
fn online_card<'a>(ui: &'a SpeechUiState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let mut items: Vec<Element<'a, Message>> = Vec::new();

    if !ui.online_available() {
        items.push(
            text("还没有安装语音插件")
                .size(14)
                .color(colors.foreground_muted)
                .into(),
        );
        items.push(store_button(colors));
        return settings_group(
            "在线模型",
            Some("在线模型来自扩展商店的语音插件；插件自己负责调用云端 API"),
            colors,
            items,
        );
    }

    // 插件下拉：多个插件只能选一个（单值配置）。
    let choices: Vec<PluginChoice> = ui
        .plugins
        .iter()
        .enumerate()
        .map(|(index, plugin)| PluginChoice {
            index,
            label: if plugin.enabled {
                plugin.name.clone()
            } else {
                format!("{}（已停用）", plugin.name)
            },
        })
        .collect();
    let current = choices.iter().find(|c| c.index == ui.plugin_index).cloned();
    let selector = pick_list(choices, current, |choice| {
        Message::SpeechPluginSelected(choice.index)
    })
    .placeholder("选择语音插件")
    .text_size(14)
    .padding([8, 10])
    .width(Length::Fixed(360.0))
    .style(move |_theme, status| pick_list_style(colors, status))
    .menu_style(crate::components::widgets::pick_list_menu_style(colors));

    let mut tools = row![selector].spacing(10).align_y(Alignment::Center);
    if ui.selected_plugin().map(|p| p.enabled).unwrap_or(false) {
        tools = tools.push(badge("已启用", colors.success, colors.success_dim));
    }
    tools = tools.push(button_secondary(
        "测试连接",
        colors,
        Message::SpeechPluginTest,
    ));
    items.push(tools.into());

    // 插件表单：schema 由插件给，值存在 plugins/config/<id>.yaml。
    if ui.busy == Some("schema") {
        items.push(
            text("正在读取插件配置…")
                .size(13)
                .color(colors.foreground_muted)
                .into(),
        );
    } else if ui.fields.is_empty() {
        items.push(
            text("该插件没有可配置项（或还没读到）")
                .size(13)
                .color(colors.foreground_muted)
                .into(),
        );
    } else {
        items.push(field_rows(ui, colors));
    }

    if let Some(message) = ui.message() {
        items.push(
            text(message.to_string())
                .size(13)
                .color(colors.primary)
                .into(),
        );
    }

    settings_group(
        "在线模型",
        Some("配置随输入生效；密钥类字段加密后存到 plugins/config/<插件>.yaml"),
        colors,
        items,
    )
}

/// 把一个插件的 `settings.schema` 渲染成表单行。
fn field_rows<'a>(ui: &'a SpeechUiState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let busy = ui.busy.is_some();
    let mut rows = column![].spacing(2).width(Length::Fill);

    for (field, value) in &ui.fields {
        // schema 的 button 语义是「测试/执行」——与剪贴板页一致，统一走测试连接。
        if field.ftype == "button" {
            rows = rows.push(settings_item(
                field.label.clone(),
                field.help_text.clone(),
                colors,
                text_button(field.label.clone(), colors, Message::SpeechPluginTest).into(),
            ));
            continue;
        }

        let key = field.key.clone();
        let on_input = (!busy).then(|| {
            let key = key.clone();
            move |value: String| Message::SpeechFieldChanged(key.clone(), value)
        });
        let input = text_input(field.placeholder.as_deref().unwrap_or(""), value)
            .size(14)
            .padding([8, 10])
            .on_input_maybe(on_input)
            .secure(field.ftype == "secret")
            .style(move |_theme, status| text_input_style(colors, status))
            .width(Length::FillPortion(2));
        rows = rows.push(settings_item(
            field.label.clone(),
            field.help_text.clone(),
            colors,
            input.into(),
        ));
    }

    rows.into()
}

/// 通用配置：语种 + 上屏方式（本地与在线共用，写进 speech.toml）。
fn common_card<'a>(ui: &'a SpeechUiState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let languages: Vec<LanguageChoice> = LANGUAGES
        .iter()
        .map(|(code, label)| LanguageChoice { code, label })
        .collect();
    let current = languages.iter().find(|c| c.code == ui.language).cloned();
    let language_picker = pick_list(languages, current, |choice| {
        Message::SpeechLanguageChanged(choice.code.to_string())
    })
    .text_size(14)
    .padding([8, 10])
    .width(Length::Fixed(200.0))
    .style(move |_theme, status| pick_list_style(colors, status))
    .menu_style(crate::components::widgets::pick_list_menu_style(colors));

    let commit_bar = tab_bar(&CommitMode::LABELS, ui.commit.index(), colors, |index| {
        Message::SpeechCommitChanged(CommitMode::from_index(index))
    });

    settings_group(
        "通用配置",
        Some("两种模式共用；写进 speech.toml，输入法进程按 mtime 热加载"),
        colors,
        vec![
            settings_item(
                "识别语种",
                Some("自动识别最省事；固定语种能减少短句误判"),
                colors,
                language_picker.into(),
            ),
            settings_item("上屏方式", Some(ui.commit.hint()), colors, commit_bar),
            settings_item(
                "当前选择",
                Some(format!(
                    "{} · 语种 {} · {}",
                    ui.mode.hint(),
                    language_label(&ui.language),
                    ui.commit.hint()
                )),
                colors,
                Space::new().width(Length::Shrink).into(),
            ),
        ],
    )
}

/// 下拉选项：插件（下标 + 文案）。
#[derive(Clone, Debug, PartialEq)]
struct PluginChoice {
    index: usize,
    label: String,
}

impl std::fmt::Display for PluginChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// 下拉选项：语种。
#[derive(Clone, Debug, PartialEq)]
struct LanguageChoice {
    code: &'static str,
    label: &'static str,
}

impl std::fmt::Display for LanguageChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label)
    }
}

/// 系统听写（WinRT，仅本页试听）——弱化处理，说明它不参与输入法。
#[cfg(windows)]
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
    let panel = panel(body, colors);

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
            settings_item("识别文本", None::<String>, colors, panel),
        ],
    )
}

/// 说明卡：怎么用、文件在哪、隐私边界（内容随模式变）。
fn help_card<'a>(mode: SpeechMode, colors: &'a ThemeColors) -> Element<'a, Message> {
    let mut items: Vec<Element<'a, Message>> = vec![settings_item(
        "怎么上屏",
        Some("打字时点候选栏的 🎙️ 开始说话，停顿时自动上屏；上屏失败会自动复制到剪贴板，按 Ctrl+V 粘贴"),
        colors,
        Space::new().width(Length::Shrink).into(),
    )];

    match mode {
        SpeechMode::Local => {
            items.push(settings_item(
                "模型文件",
                Some(
                    "模型目录结构随平台而异；删除模型目录即等于未下载，导入的模型与商店下载的等价",
                ),
                colors,
                Space::new().width(Length::Shrink).into(),
            ));
            items.push(settings_item(
                "GPU 加速",
                Some("安装带 GPU 后端的安装包即优先用显卡推理；机器上缺运行库时自动回退 CPU（日志里有提示），不会报错"),
                colors,
                Space::new().width(Length::Shrink).into(),
            ));
            items.push(settings_item(
                "隐私",
                Some("语音识别完全在本机进行，音频不出电脑；只有点「下载」时会联网取模型文件"),
                colors,
                Space::new().width(Length::Shrink).into(),
            ));
        }
        SpeechMode::Online => {
            items.push(settings_item(
                "插件配置",
                Some("配置项由插件的 settings.schema 决定，值加密存放在 plugins/config/<插件 id>.yaml；换插件不会丢配置"),
                colors,
                Space::new().width(Length::Shrink).into(),
            ));
            items.push(settings_item(
                "隐私",
                Some(
                    "在线模式下音频会发送到插件配置的云端服务，请先确认该服务的隐私政策与计费方式",
                ),
                colors,
                Space::new().width(Length::Shrink).into(),
            ));
        }
    }

    settings_group("使用说明", None::<String>, colors, items)
}

/// 浅色面板（试听结果 / 识别文本共用）。
fn panel<'a>(body: Element<'a, Message>, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    container(body)
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
        })
        .into()
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
