//! 导入词库子页面：已导入列表 + 导入弹窗。
//!
//! 版式对齐 Android 的「词库管理 → 导入词库」：页面主体是**已经导入了什么**，
//! 导入动作放在页头主按钮上，点开弹窗才出现「选文件 → 解析 → 挂方案」表单。
//! 表单常驻页面会把列表挤到折叠线以下，也会让「只是来关停某本词库」的人
//! 每次都穿过一张大表单。
use crate::components::settings::settings_group;
use crate::components::widgets::{
    button_danger, button_disabled, button_primary, button_secondary, medium, semibold, text_button,
    text_input_style,
};
use crate::state::{DictImportState, Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Background, Color, Length};

/// 文件类型提示（输入框占位文案与说明共用）。
const FILE_HINT: &str = ".scel / .bdict / .dict / .bin / .txt";

/// 子页面主体：已导入词库列表 + 脚注。
pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> iced::Element<'a, Message> {
    let d = &settings.dict_import;
    column![imported_group(d, colors), footnote(colors)]
        .spacing(16)
        .width(Length::Fill)
        .into()
}

/// 页头右侧主操作：导入词库（打开弹窗）。
pub fn header_action<'a>(
    d: &DictImportState,
    colors: &'a ThemeColors,
) -> iced::Element<'a, Message> {
    if d.busy.is_some() {
        button_disabled("导入新词库", colors)
    } else {
        button_primary("导入新词库", colors, Message::DictImportDialogOpen).into()
    }
}

// ---- 导入弹窗 ----

/// 导入弹窗：文件 → 解析结果 → 挂载方案 → 取消 / 导入并部署。
///
/// 表单区固定高度并允许滚动：解析结果的样例条数、方案胶囊折几行都会变，
/// 不锁高会推着弹窗外框上下跳。
pub fn dialog<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> iced::Element<'a, Message> {
    let d = &settings.dict_import;

    let head = column![
        text("导入词库".to_string())
            .size(16)
            .font(semibold())
            .color(colors.foreground),
        text("把其他输入法的词库转换成 Rime 的 .dict.yaml，并挂到拼音方案上。".to_string())
            .size(12)
            .color(colors.foreground_muted),
    ]
    .spacing(4)
    .width(Length::Fill);

    let mut form = column![].spacing(14).width(Length::Fill);
    form = form.push(file_picker(d, colors));
    if d.parsing {
        form = form.push(hint_line("正在解析…".to_string(), colors.foreground_muted));
    }
    if let Some(error) = &d.preview_error {
        form = form.push(hint_line(error.clone(), colors.error));
    }
    if let Some(preview) = &d.preview {
        form = form.push(preview_block(preview, colors));
    }
    form = form.push(scheme_block(d, colors));

    let mut actions = row![].spacing(8).align_y(Alignment::Center);
    actions = actions.push(text_button(
        "取消",
        colors,
        Message::DictImportDialogCancel,
    ));
    actions = actions.push(Space::new().width(Length::Fill));
    actions = actions.push(import_button(d, colors));

    column![
        head,
        // 解析结果出来之前表单只有百来像素，锁死高度会在弹窗里留一大片
        // 空白；有预览（多了样例与方案胶囊）才限制高度并允许滚动。
        scrollable(form)
            .height(if d.preview.is_some() {
                Length::Fixed(300.0)
            } else {
                Length::Shrink
            })
            .width(Length::Fill),
        divider(colors),
        actions,
    ]
    .spacing(12)
    .width(Length::Fill)
    .into()
}

/// 弹窗里的「词库文件」：说明 + 浏览 + 路径输入 + 解析。
fn file_picker<'a>(d: &'a DictImportState, colors: &'a ThemeColors) -> iced::Element<'a, Message> {
    let locked = d.is_busy();

    let pick_button: iced::Element<'a, Message> = if locked {
        button_disabled("浏览…", colors)
    } else {
        button_secondary("浏览…", colors, Message::DictImportBrowse).into()
    };
    let parse_button: iced::Element<'a, Message> = if locked {
        button_disabled("解析", colors)
    } else {
        button_secondary("解析", colors, Message::DictImportParse).into()
    };

    column![
        row![
            column![
                text("词库文件".to_string())
                    .size(13)
                    .font(medium())
                    .color(colors.foreground),
                text(format!("支持 {FILE_HINT}"))
                    .size(12)
                    .color(colors.foreground_muted),
            ]
            .spacing(2)
            .width(Length::Fill),
            pick_button,
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        row![
            text_input(FILE_HINT, &d.path)
                .size(13)
                .padding([8, 10])
                .on_input(Message::DictImportPathChanged)
                .style(move |_t, status| text_input_style(colors, status))
                .width(Length::Fill),
            parse_button,
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    ]
    .spacing(8)
    .width(Length::Fill)
    .into()
}

/// 解析结果：格式 / 统计 + 前几条词条。
fn preview_block<'a>(
    preview: &'a crate::state::LexiconPreview,
    colors: &'a ThemeColors,
) -> iced::Element<'a, Message> {
    let mut body = column![].spacing(3).width(Length::Fill);
    body = body.push(
        row![
            text(preview.format_name.clone())
                .size(13)
                .font(medium())
                .color(colors.foreground),
            Space::new().width(Length::Fill),
            text(format!(
                "{} 条 · 跳过 {} · 重复 {}",
                preview.entry_count, preview.skipped, preview.duplicates
            ))
            .size(12)
            .color(colors.foreground_muted),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );
    for (word, code) in &preview.samples {
        body = body.push(hint_line(
            format!("{word}    {code}"),
            colors.foreground_muted,
        ));
    }
    if preview.entry_count > preview.samples.len() {
        body = body.push(hint_line("…".to_string(), colors.foreground_faint));
    }
    container(body)
        .width(Length::Fill)
        .padding([10, 12])
        .style(move |_| container::Style {
            background: Some(Background::Color(colors.surface_variant)),
            border: iced::border::Border {
                color: colors.border,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// 挂载方案：整行铺开，胶囊可换行。
fn scheme_block<'a>(d: &'a DictImportState, colors: &'a ThemeColors) -> iced::Element<'a, Message> {
    let mut block = column![].spacing(8).width(Length::Fill);
    let mut title_row = row![text("挂载方案".to_string())
        .size(13)
        .font(medium())
        .color(colors.foreground)]
    .spacing(8)
    .align_y(Alignment::Center);
    if !d.compatible.is_empty() && !d.selected.is_empty() {
        title_row = title_row.push(Space::new().width(Length::Fill));
        title_row = title_row.push(
            text(format!("已选 {} 个", d.selected.len()))
                .size(12)
                .color(colors.foreground_muted),
        );
    }
    block = block.push(title_row);

    if d.compatible.is_empty() {
        return block
            .push(hint_line(
                "当前没有可挂拼音词库的方案（需要 script_translator）。不选也没关系：导入后只生成 .dict.yaml，可以在方案 custom.yaml 里自己引用。"
                    .to_string(),
                colors.foreground_muted,
            ))
            .into();
    }

    block = block.push(hint_line(
        "可多选；不选就只生成 .dict.yaml".to_string(),
        colors.foreground_muted,
    ));
    let mut chips = row![].spacing(8);
    for (id, name) in &d.compatible {
        let active = d.selected.iter().any(|selected| selected == id);
        chips = chips.push(scheme_chip(
            name.clone(),
            active,
            colors,
            Message::DictImportToggleScheme(id.clone()),
        ));
    }
    block.push(chips.wrap()).into()
}

/// 弹窗主操作：解析过才可点。
fn import_button<'a>(d: &DictImportState, colors: &'a ThemeColors) -> iced::Element<'a, Message> {
    if d.busy == Some("import") {
        button_disabled("正在导入…", colors)
    } else if d.preview.is_some() && !d.is_busy() {
        button_primary("导入并部署", colors, Message::DictImportSubmit).into()
    } else {
        button_disabled("导入并部署", colors)
    }
}

/// 可点选的方案胶囊。
fn scheme_chip<'a>(
    label: String,
    active: bool,
    colors: &ThemeColors,
    on_press: Message,
) -> iced::Element<'a, Message> {
    let colors = *colors;
    button(
        text(label)
            .size(12)
            .font(medium())
            .color(if active {
                colors.on_primary
            } else {
                colors.foreground
            }),
    )
    .on_press(on_press)
    .padding([5, 10])
    .style(move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let background = if active {
            if hovered {
                colors.primary_hover
            } else {
                colors.primary
            }
        } else if hovered {
            colors.surface_hover
        } else {
            colors.surface_variant
        };
        button::Style {
            background: Some(Background::Color(background)),
            text_color: if active {
                colors.on_primary
            } else {
                colors.foreground
            },
            border: iced::border::Border {
                color: if active { colors.primary } else { colors.border },
                width: 1.0,
                radius: 999.0.into(),
            },
            ..button::Style::default()
        }
    })
    .into()
}

// ---- 已导入列表 ----

fn imported_group<'a>(d: &'a DictImportState, colors: &'a ThemeColors) -> iced::Element<'a, Message> {
    let description = if d.items.is_empty() {
        "还没有导入过词库。".to_string()
    } else {
        format!(
            "共 {} 本；停用只摘掉 packs，文件与记录都保留。",
            d.items.len()
        )
    };

    let mut body: Vec<iced::Element<'a, Message>> = Vec::new();
    if !d.items.is_empty() {
        // 操作结果（启用/停用/删除/改挂、导入失败）贴着列表顶上，不沉底。
        if let Some(notice) = &d.notice {
            body.push(hint_line(notice.clone(), colors.primary));
        }
        if let Some(error) = &d.error {
            body.push(hint_line(error.clone(), colors.error));
        }
    }
    if d.items.is_empty() {
        body.push(
            container(
                text("点右上角「导入新词库」，选一个文件解析后导入".to_string())
                    .size(13)
                    .color(colors.foreground_muted),
            )
            .width(Length::Fill)
            .padding([12, 0])
            .into(),
        );
    } else {
        body.push(table_header(colors));
        for (index, item) in d.items.iter().enumerate() {
            body.push(imported_row(d, item, index, colors));
        }
    }

    settings_group("已导入词库", Some(description), colors, body)
}

/// 一行已导入词库：名称/来源 + 状态 + 挂载方案 + 操作（改挂编辑器就地展开）。
fn imported_row<'a>(
    d: &'a DictImportState,
    item: &'a xime_config::imported_dict::ImportedDict,
    index: usize,
    colors: &'a ThemeColors,
) -> iced::Element<'a, Message> {
    let editing = d.rebind_target.as_deref() == Some(item.id.as_str());

    let mut info = column![
        text(item.display_name.clone())
            .size(14)
            .font(medium())
            .color(colors.foreground),
        text(format!(
            "{} · {} · {} 条",
            item.format_name, item.code_system, item.entry_count
        ))
        .size(12)
        .color(colors.foreground_muted),
    ]
    .spacing(2)
    .width(Length::Fill);
    if editing {
        info = info.push(rebind_editor(d, colors));
    }

    let mut actions = row![].spacing(4).align_y(Alignment::Center);
    if editing {
        actions = actions.push(text_button("取消", colors, Message::DictImportRebindCancel));
        actions = actions.push(button_primary(
            "保存",
            colors,
            Message::DictImportRebindSubmit,
        ));
    } else if d.busy.is_some() {
        actions = actions.push(text("处理中…").size(12).color(colors.foreground_muted));
    } else if d.delete_confirm.as_deref() == Some(item.id.as_str()) {
        actions = actions.push(text_button("取消", colors, Message::DictImportDeleteCancel));
        actions = actions.push(
            button_danger(
                "确认删除",
                colors,
                Message::DictImportDeleteConfirm(item.id.clone()),
            )
            .padding([4, 10]),
        );
    } else {
        // 未挂载时不能启用：挂了方案才谈得上生效。
        if item.attached_schemas.is_empty() {
            actions = actions.push(text("先挂方案").size(12).color(colors.foreground_muted));
        } else {
            let (label, enabled) = if item.enabled {
                ("停用", false)
            } else {
                ("启用", true)
            };
            actions = actions.push(text_button(
                label,
                colors,
                Message::DictImportSetEnabled(item.id.clone(), enabled),
            ));
        }
        actions = actions.push(text_button(
            "更改方案",
            colors,
            Message::DictImportRebindOpen(item.id.clone()),
        ));
        actions = actions.push(text_button(
            "删除",
            colors,
            Message::DictImportDeleteRequest(item.id.clone()),
        ));
    }

    let line = row![
        info,
        status_badge(d, item, colors),
        cell(d.attach_label(item), 170.0, colors),
        container(actions)
            .width(Length::Fixed(210.0))
            .align_x(iced::alignment::Horizontal::Right),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let tinted = index % 2 == 1;
    container(line)
        .width(Length::Fill)
        .padding([8, 10])
        .style(move |_| container::Style {
            background: if tinted {
                Some(Background::Color(colors.surface_variant))
            } else {
                None
            },
            ..container::Style::default()
        })
        .into()
}

/// 状态徽章：未挂载 / 已停用 / 已生效 / 待部署（对齐 Android 的四种状态）。
fn status_badge<'a>(
    d: &DictImportState,
    item: &xime_config::imported_dict::ImportedDict,
    colors: &ThemeColors,
) -> iced::Element<'a, Message> {
    let badge = if item.attached_schemas.is_empty() {
        crate::components::widgets::badge("未挂载", colors.on_error, colors.error_dim)
    } else if !item.enabled {
        crate::components::widgets::badge("已停用", colors.foreground_muted, colors.surface_variant)
    } else if d.compiled.get(&item.id).copied().unwrap_or(false) {
        crate::components::widgets::badge("已生效", colors.on_primary, colors.primary_dim)
    } else {
        crate::components::widgets::badge("待部署", colors.on_error, colors.error_dim)
    };
    container(badge)
        .width(Length::Fixed(70.0))
        .align_x(iced::alignment::Horizontal::Left)
        .into()
}

/// 「更改方案」编辑器：勾选方案（就地展开在词库名下）。
fn rebind_editor<'a>(d: &'a DictImportState, colors: &'a ThemeColors) -> iced::Element<'a, Message> {
    if d.compatible.is_empty() {
        return hint_line(
            "当前没有可挂拼音词库的方案".to_string(),
            colors.foreground_muted,
        );
    }
    let mut chips = row![].spacing(8);
    for (id, name) in &d.compatible {
        let active = d.rebind_selected.iter().any(|selected| selected == id);
        chips = chips.push(scheme_chip(
            name.clone(),
            active,
            colors,
            Message::DictImportRebindToggle(id.clone()),
        ));
    }
    column![
        hint_line("勾选要挂载的方案".to_string(), colors.foreground_muted),
        chips.wrap(),
    ]
    .spacing(6)
    .padding([6, 0])
    .into()
}

fn table_header(colors: &ThemeColors) -> iced::Element<'_, Message> {
    let head = row![
        text("名称 / 来源".to_string())
            .size(12)
            .font(medium())
            .color(colors.foreground_muted)
            .width(Length::Fill),
        container(
            text("状态".to_string())
                .size(12)
                .font(medium())
                .color(colors.foreground_muted),
        )
        .width(Length::Fixed(70.0)),
        head_cell("挂载方案", 170.0, colors),
        container(
            text("操作".to_string())
                .size(12)
                .font(medium())
                .color(colors.foreground_muted),
        )
        .width(Length::Fixed(210.0))
        .align_x(iced::alignment::Horizontal::Right),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    container(head).width(Length::Fill).padding([0, 10]).into()
}

fn cell<'a>(value: String, width: f32, colors: &'a ThemeColors) -> iced::Element<'a, Message> {
    text(value)
        .size(13)
        .color(colors.foreground)
        .width(Length::Fixed(width))
        .into()
}

fn head_cell<'a>(label: &'a str, width: f32, colors: &'a ThemeColors) -> iced::Element<'a, Message> {
    text(label.to_string())
        .size(12)
        .font(medium())
        .color(colors.foreground_muted)
        .width(Length::Fixed(width))
        .into()
}

fn footnote(colors: &ThemeColors) -> iced::Element<'_, Message> {
    hint_line(
        "解析只读文件；导入后会自动部署，部署完成后徽章会变成「已生效」。".to_string(),
        colors.foreground_muted,
    )
}

/// 弹窗内分隔线（表单与操作按钮之间）。
fn divider(colors: &ThemeColors) -> iced::Element<'static, Message> {
    let colors = *colors;
    container(Space::new().width(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .padding([0, 0])
        .style(move |_| container::Style {
            background: Some(Background::Color(colors.border)),
            ..container::Style::default()
        })
        .into()
}

fn hint_line(value: String, color: Color) -> iced::Element<'static, Message> {
    text(value).size(12).color(color).width(Length::Fill).into()
}
