#![cfg(feature = "clipboard-page")]
use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{
    label, medium, modal_dialog, semibold, switch, text_button, text_input_style,
};
#[cfg_attr(not(target_os = "linux"), allow(unused_imports))]
use crate::components::widgets::{badge, button_primary, button_secondary};
use crate::state::{CLIPBOARD_PAGE_SIZE, ClipboardState, Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{button, column, container, pick_list, row, text, text_input};
use iced::{border, Background, Border, Color, Element, Length};

/// 下拉选项：id 参与相等性比较，避免同名插件选中错乱。
#[derive(Clone, PartialEq)]
struct PluginOption {
    id: String,
    name: String,
}

impl std::fmt::Display for PluginOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

// 本地同步服务器（xime-sync-server）：Linux 桌面部署形态。
// Windows 端输入法内嵌插件同步，不随包分发该服务，隐藏以免误导。
#[cfg(target_os = "linux")]
fn server_groups<'a>(c: &'a ClipboardState, colors: &'a ThemeColors) -> Vec<Element<'a, Message>> {
    // 服务器状态
    let status = if c.running {
        badge("运行中", Color::from_rgb8(0x2E, 0xA0, 0x7D), Color::WHITE)
    } else {
        badge("已停止", Color::from_rgb8(0xE5, 0x8F, 0x2A), Color::WHITE)
    };

    // 操作按钮（运行中 → 停止/重启；停止 → 启动）
    let actions = if c.running {
        row![
            button_primary("停止", colors, Message::ServerStop),
            button_secondary("重启", colors, Message::ServerRestart),
        ]
        .spacing(8)
    } else {
        row![button_primary("启动", colors, Message::ServerStart)].spacing(8)
    };

    // 配置编辑：未运行时允许修改（运行中修改需重启生效）
    let editable = !c.running;
    let addr_input = text_input("0.0.0.0:8443", &c.server_addr)
        .on_input_maybe(editable.then_some(Message::ServerAddrChanged))
        .style(move |_t, s| text_input_style(colors, s))
        .width(Length::FillPortion(2));
    let user_input = text_input("xime", &c.username)
        .on_input_maybe(editable.then_some(Message::ServerUsernameChanged))
        .style(move |_t, s| text_input_style(colors, s))
        .width(Length::FillPortion(2));
    let pass_input = text_input("", &c.password)
        .on_input_maybe(editable.then_some(Message::ServerPasswordChanged))
        .secure(true)
        .style(move |_t, s| text_input_style(colors, s))
        .width(Length::FillPortion(2));

    let mut out = vec![
        settings_group(
            "同步服务器",
            Some("本机剪切板同步服务（xime-sync-server），供其他设备同步剪切板内容"),
            colors,
            vec![
                settings_item(
                    "监听地址",
                    Some("格式 地址:端口，默认 0.0.0.0:8443"),
                    colors,
                    addr_input.into(),
                ),
                settings_item("用户名", Some("客户端连接认证"), colors, user_input.into()),
                settings_item(
                    "密码",
                    Some("客户端连接认证，明文保存于本地配置文件 (0600)"),
                    colors,
                    pass_input.into(),
                ),
                settings_item(
                    "数据目录",
                    Some("服务器存储目录（历史记录等）"),
                    colors,
                    row![
                        text(&c.data_dir).size(14).color(colors.foreground_muted),
                        text_button("打开", colors, Message::OpenSyncDataDir),
                    ]
                    .spacing(8)
                    .into(),
                ),
            ],
        ),
        settings_group(
            "服务器状态",
            None::<String>,
            colors,
            vec![
                settings_item("运行状态", None::<String>, colors, row![status].into()),
                settings_item("操作", c.status_message.as_deref(), colors, actions.into()),
            ],
        ),
    ];

    if c.running {
        out.push(settings_group(
            "剪贴板历史",
            Some("管理剪贴板历史记录"),
            colors,
            vec![settings_item(
                "启用剪贴板历史",
                Some("记录复制历史以便快速粘贴"),
                colors,
                label("开发中", colors),
            )],
        ));
    }
    out
}

#[cfg(not(target_os = "linux"))]
fn server_groups<'a>(
    _c: &'a ClipboardState,
    _colors: &'a ThemeColors,
) -> Vec<Element<'a, Message>> {
    Vec::new()
}

/// 剪贴板同步 Tab：插件方案（全平台，对齐 Android）+ 本地同步服务器（仅 Linux）。
fn sync_groups<'a>(
    settings: &'a SettingsState,
    colors: &'a ThemeColors,
) -> Vec<Element<'a, Message>> {
    let sp = &settings.sync_plugin;
    let mut sync_items: Vec<Element<'a, Message>> = Vec::new();

    // 启用开关。
    sync_items.push(settings_item(
        "启用剪贴板同步",
        Some("通过同步插件与远端设备双向同步（与 Android 插件通用）"),
        colors,
        switch(sp.enabled, colors, Message::SyncPluginEnabled),
    ));

    // 插件选择：列出全部已安装的 clipboard_sync 插件（对齐 Android 端，
    // 停用的也显示——选中即启用并通知 daemon 重载）。
    if sp.plugins.is_empty() {
        sync_items.push(settings_item(
            "同步插件",
            Some("未安装 clipboard_sync 插件；请将插件目录放入 plugins/ 后重启设置程序"),
            colors,
            label("", colors),
        ));
    } else {
        let options: Vec<PluginOption> = sp
            .plugins
            .iter()
            .map(|p| PluginOption {
                id: p.id.clone(),
                name: if p.enabled {
                    p.name.clone()
                } else {
                    format!("{}（未启用）", p.name)
                },
            })
            .collect();
        let current = sp.plugins.get(sp.plugin_index).map(|p| PluginOption {
            id: p.id.clone(),
            name: p.name.clone(),
        });
        sync_items.push(settings_item(
            "同步插件",
            Some("选择承载剪贴板同步的插件，选中后立即生效"),
            colors,
            pick_list(options, current, |chosen| {
                Message::SyncPluginSelected(
                    sp.plugins
                        .iter()
                        .position(|p| p.id == chosen.id)
                        .unwrap_or(0),
                )
            })
            .into(),
        ));
    }

    if sp.busy == Some("schema") {
        sync_items.push(settings_item(
            "插件配置",
            None::<String>,
            colors,
            container(
                text("正在加载插件配置…")
                    .size(13)
                    .color(colors.foreground_muted),
            )
            .width(Length::Fill)
            .into(),
        ));
    } else {
        for (field, value) in &sp.fields {
            if field.ftype == "button" {
                sync_items.push(settings_item(
                    field.label.clone(),
                    field.help_text.clone(),
                    colors,
                    text_button(field.label.clone(), colors, Message::SyncPluginTest).into(),
                ));
                continue;
            }
            let key = field.key.clone();
            let on_input = (!sp.busy.is_some()).then(|| {
                let key = key.clone();
                move |v: String| Message::SyncPluginFieldChanged(key.clone(), v)
            });
            let input = text_input(field.placeholder.as_deref().unwrap_or(""), value)
                .on_input_maybe(on_input)
                .secure(field.ftype == "secret")
                .style(move |_t, s| text_input_style(colors, s))
                .width(Length::FillPortion(2));
            sync_items.push(settings_item(
                field.label.clone(),
                field.help_text.clone(),
                colors,
                input.into(),
            ));
        }
    }
    if let Some(msg) = &sp.message {
        sync_items.push(settings_item(
            "状态",
            None::<String>,
            colors,
            text(msg).size(13).color(colors.foreground_muted).into(),
        ));
    }

    let plugin_group = settings_group(
        "剪贴板同步（插件）",
        Some("启用后输入法在后台自动推送/拉取；协议由插件承载，多端使用同一插件即可互通"),
        colors,
        sync_items,
    );

    let mut groups = vec![plugin_group];
    groups.extend(server_groups(&settings.clipboard, colors));
    groups
}

/// 剪贴板页：Tabs 切换「剪贴板历史 / 快捷发送 / 剪贴板同步」。
pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let tab = settings.clipboard_tab;
    let mut groups: Vec<Element<'a, Message>> = vec![clipboard_header(settings, colors)];
    match tab {
        0 => groups.extend(history_groups(settings, colors)),
        1 => groups.extend(quick_send_groups(settings, colors)),
        _ => groups.extend(sync_groups(settings, colors)),
    }
    let page = settings_page("剪贴板", colors, groups);
    let dialog = if settings.quick_send.dialog_open {
        Some(quick_send_dialog(settings, colors))
    } else {
        None
    };
    modal_dialog(page, dialog, colors)
}

/// 列表卡片按钮（剪贴板历史 / 快捷发送共用样式）：
/// 点击选中，选中态主色边框 + 背景加深 + hover 反馈。
fn card_button<'a>(
    content: Element<'a, Message>,
    is_selected: bool,
    colors: &ThemeColors,
    on_press: Message,
) -> iced::widget::Button<'a, Message> {
    let colors_copy = *colors;
    button(
        container(content)
            .width(Length::Fill)
            .padding([10, 12]),
    )
    .width(Length::Fill)
    .padding(0)
    .style(move |_t, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: Some(Background::Color(Color {
                a: if is_selected {
                    0.10
                } else if hovered {
                    0.08
                } else {
                    0.06
                },
                ..colors_copy.foreground
            })),
            text_color: colors_copy.foreground,
            border: Border {
                color: if is_selected {
                    colors_copy.primary
                } else {
                    Color::TRANSPARENT
                },
                width: if is_selected { 1.5 } else { 0.0 },
                radius: border::radius(8.0),
            },
            ..button::Style::default()
        }
    })
    .on_press(on_press)
}

/// 分页条：上一页 / 第 x / y 页 / 下一页（首页、末页对应按钮置灰禁用）。
fn page_bar<'a>(
    page: usize,
    total_pages: usize,
    colors: &ThemeColors,
    prev_msg: Message,
    next_msg: Message,
) -> Element<'a, Message> {
    let c = *colors;
    let muted = c.foreground_muted;
    let nav = |label: String, enabled: bool, msg: Message| -> iced::widget::Button<'a, Message> {
        let accent = if enabled { c.primary } else { muted };
        button(
            text(label)
                .size(13)
                .font(medium())
                .color(accent),
        )
        .padding([6, 12])
        .on_press_maybe(enabled.then_some(msg))
        .style(move |_t, status| {
            let hovered =
                enabled && matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: if hovered {
                    Some(Background::Color(Color {
                        a: 0.08,
                        ..c.foreground
                    }))
                } else {
                    None
                },
                text_color: accent,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: border::radius(8.0),
                },
                ..button::Style::default()
            }
        })
    };
    row![
        nav("上一页".to_string(), page > 0, prev_msg),
        text(format!("第 {} / {} 页", page + 1, total_pages))
            .size(13)
            .color(muted),
        nav("下一页".to_string(), page + 1 < total_pages, next_msg),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .into()
}

/// 添加快捷发送弹窗内容。
fn quick_send_dialog<'a>(
    settings: &'a SettingsState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let q = &settings.quick_send;
    let content_input = text_input("要快速发送的内容", &q.draft_content)
        .on_input(|v| Message::QuickSendContentChanged(v))
        .style(move |_t, s| text_input_style(colors, s))
        .width(Length::Fill);
    let code_input = text_input("如 dh", &q.draft_code)
        .on_input(|v| Message::QuickSendCodeChanged(v))
        .style(move |_t, s| text_input_style(colors, s))
        .width(Length::Fill);
    column![
        text("添加快捷发送")
            .size(16)
            .font(semibold())
            .color(colors.foreground),
        text("内容").size(13).color(colors.foreground_muted),
        content_input,
        text("触发编码（输入该编码前缀时条目进入候选栏，留空不设）")
            .size(13)
            .color(colors.foreground_muted),
        code_input,
        row![
            button_secondary("取消", colors, Message::QuickSendCancel),
            button_primary("添加", colors, Message::QuickSendAdd),
        ]
        .spacing(8)
    ]
    .spacing(8)
    .into()
}

/// 页头：Tab 栏居左、当前页操作按钮居右（space-between）。
fn clipboard_header<'a>(
    settings: &'a SettingsState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let tab = settings.clipboard_tab;
    let tabs: Element<'a, Message> = clipboard_tab_bar(tab, colors);
    let mut header = row![tabs].align_y(iced::Alignment::Center);
    match tab {
        0 => {
            header = header.push(iced::widget::Space::new().width(Length::Fill));
            header = header.push(
                row![
                    text_button("刷新", colors, Message::ClipboardHistoryRefresh),
                    text_button("清空历史", colors, Message::ClearClipboardHistory),
                ]
                .spacing(8),
            );
        }
        1 => {
            header = header.push(iced::widget::Space::new().width(Length::Fill));
            header = header.push(text_button("添加", colors, Message::QuickSendOpen));
        }
        _ => {}
    }
    header.into()
}

/// 页内 Tab 栏（样式对齐扩展商店页 tab_bar）。
fn clipboard_tab_bar<'a>(active: usize, colors: &'a ThemeColors) -> Element<'a, Message> {
    let labels = ["剪贴板历史", "快捷发送", "剪贴板同步"];
    let mut bar = row![].spacing(2).padding(2);

    for (i, label_text) in labels.iter().enumerate() {
        let is_active = i == active;
        let label_text = *label_text;
        bar = bar.push(
            button(text(label_text).size(14).color(if is_active {
                colors.primary
            } else {
                colors.foreground_muted
            }))
            .padding([7, 16])
            .style(move |_theme, status| {
                let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: if is_active || hovered {
                        Some(Background::Color(colors.surface))
                    } else {
                        None
                    },
                    text_color: if is_active {
                        colors.primary
                    } else {
                        colors.foreground_muted
                    },
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: border::radius(8.0),
                    },
                    ..button::Style::default()
                }
            })
            .on_press(Message::ClipboardTab(i)),
        );
    }

    container(bar)
        .width(Length::Shrink)
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

// ------------------------------------------------------------------
// 剪贴板历史（server 剪贴板工作线程持久化 → clipboard.db）
// ------------------------------------------------------------------

const HISTORY_TEXT_DISPLAY_MAX: usize = 60;

fn history_groups<'a>(
    settings: &'a SettingsState,
    colors: &'a ThemeColors,
) -> Vec<Element<'a, Message>> {
    let h = &settings.clip_history;
    if h.items.is_empty() {
        return vec![settings_group(
            "剪贴板历史",
            Some("复制的内容会记录在这里（最近 50 条）；当前暂无记录，复制任意内容后点右上角「刷新」"),
            colors,
            vec![settings_item(
                "暂无记录",
                None::<String>,
                colors,
                label("", colors),
            )],
        )];
    }

    let mut items: Vec<Element<'a, Message>> = Vec::new();
    // 当前页切片（最新在前）
    let start = h.page.min(h.total_pages() - 1) * CLIPBOARD_PAGE_SIZE;
    let end = (start + CLIPBOARD_PAGE_SIZE).min(h.items.len());
    let mut grid = iced::widget::Grid::new().columns(2).spacing(8).height(Length::Shrink);
    for entry in h.items[start..end].iter() {
        let mut display = entry.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if display.chars().count() > HISTORY_TEXT_DISPLAY_MAX {
            display = display.chars().take(HISTORY_TEXT_DISPLAY_MAX).collect::<String>() + "…";
        }
        let is_selected = h.selected == Some(entry.id);
        // 卡片内容：文本 +（选中时）操作按钮
        let mut card = column![text(display).size(13).color(colors.foreground)].spacing(8);
        if is_selected {
            card = card.push(
                row![
                    text_button("添加到快捷发送", colors, Message::QuickSendFromHistory(entry.id)),
                    text_button("删除", colors, Message::ClipboardHistoryRemove(entry.id)),
                ]
                .spacing(8),
            );
        }
        grid = grid.push(card_button(
            card.into(),
            is_selected,
            colors,
            Message::ClipboardHistorySelected(entry.id),
        ));
    }
    items.push(grid.into());
    if h.total_pages() > 1 {
        items.push(page_bar(
            h.page,
            h.total_pages(),
            colors,
            Message::ClipboardHistoryPrevPage,
            Message::ClipboardHistoryNextPage,
        ));
    }
    vec![settings_group(
        "剪贴板历史",
        Some("复制的内容会记录在这里（最近 50 条，随输入法后台自动记录）"),
        colors,
        items,
    )]
}

// ------------------------------------------------------------------
// 快捷发送（clipboard.db isQuickSend=1 子集，对齐 Android QuickSendItem）
// ------------------------------------------------------------------

fn quick_send_groups<'a>(
    settings: &'a SettingsState,
    colors: &'a ThemeColors,
) -> Vec<Element<'a, Message>> {
    let q = &settings.quick_send;
    let mut items: Vec<Element<'a, Message>> = Vec::new();

    // 当前页切片
    let start = q.page.min(q.total_pages() - 1) * CLIPBOARD_PAGE_SIZE;
    let end = (start + CLIPBOARD_PAGE_SIZE).min(q.items.len());
    let mut grid = iced::widget::Grid::new().columns(2).spacing(8).height(Length::Shrink);
    for entry in q.items[start..end].iter() {
        let mut summary = entry.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if summary.chars().count() > 60 {
            summary = summary.chars().take(60).collect::<String>() + "…";
        }
        if !entry.code.is_empty() {
            summary = format!("编码 {} · {}", entry.code, summary);
        }
        let title: String = entry.text.chars().take(16).collect();
        let is_selected = q.selected == Some(entry.id);
        // 与历史卡同款：点击选中，删除按钮仅选中时出现
        let mut card = column![
            text(title)
                .size(14)
                .font(semibold())
                .color(colors.foreground),
            text(summary).size(13).color(colors.foreground_muted),
        ]
        .spacing(6);
        if is_selected {
            card = card.push(
                row![text_button("删除", colors, Message::QuickSendRemove(entry.id))].spacing(8),
            );
        }
        grid = grid.push(card_button(
            card.into(),
            is_selected,
            colors,
            Message::QuickSendSelected(entry.id),
        ));
    }
    items.push(grid.into());
    if q.total_pages() > 1 {
        items.push(page_bar(
            q.page,
            q.total_pages(),
            colors,
            Message::QuickSendPrevPage,
            Message::QuickSendNextPage,
        ));
    }
    if q.items.is_empty() {
        items.push(settings_item(
            "暂无快捷发送内容",
            Some("点击右上角「添加」录入常用短语；设置触发编码后，输入编码前缀即可让条目进入候选栏"),
            colors,
            label("", colors),
        ));
    }

    vec![settings_group(
        "快捷发送",
        Some("常用短语列表，保存在本机（clipboard.db 与多端同构），多端不互相同步"),
        colors,
        items,
    )]
}
