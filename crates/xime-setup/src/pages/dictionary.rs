//! 词典管理页：用户词典 + 快捷短语（rime 操作经 IPC 在 server 执行）。
//!
//! 版式是「页内 Tab + 数据表格」，不是设置表单：词典浏览/编辑类页面
//! （对齐小狼毫词典管理对话框、本仓库剪贴板页）的信息结构是
//! 「工具行（选择器 + 搜索 + 主操作）→ 表格（列头对齐 + 斑马纹行）→
//! 页脚（统计 + 翻页 / 主操作）」，每本词典一行按钮的表单式布局装不下
//! 这些数据，也不该用它。
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::Element;

#[cfg(any(windows, feature = "dict-page"))]
use crate::components::widgets::{
    button_danger, button_disabled, button_primary, card_style, medium, modal_dialog, semibold,
    text_button, text_input_style,
};
#[cfg(any(windows, feature = "dict-page"))]
use crate::state::{CustomPhraseState, DICT_ENTRIES_PAGE_SIZE};
#[cfg(any(windows, feature = "dict-page"))]
use iced::widget::{button, column, container, pick_list, row, text, text_input, Space};
#[cfg(any(windows, feature = "dict-page"))]
use iced::{border, Alignment, Background, Border, Color, Length};

/// Windows：真实词典管理（rime 用户词典经 IPC 操作）。
#[cfg(any(windows, feature = "dict-page"))]
pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let d = &settings.dict_manage;

    // 页头：标题居左、全局操作居右（同输入方案页的 header 版式）。
    let header = row![
        text("词典管理")
            .size(20)
            .font(semibold())
            .color(colors.foreground),
        Space::new().width(Length::Fill),
        text_button("刷新", colors, Message::DictRefresh),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let mut content = column![header, tab_bar(d.tab, colors)]
        .spacing(16)
        .padding(20)
        .width(Length::Fill);

    content = content.push(if d.tab == 0 {
        user_dict_view(d, colors)
    } else {
        phrase_view(settings, colors)
    });

    let dialog = d
        .browse
        .add_dialog
        .as_ref()
        .map(|draft| entry_add_dialog(draft, colors))
        .or_else(|| {
            d.phrase
                .as_ref()
                .and_then(|phrase| phrase.dialog.as_ref())
                .map(|dialog| phrase_dialog_view(dialog, colors))
        });
    modal_dialog(content.into(), dialog, colors)
}

/// 页内 Tab 栏（样式对齐剪贴板页 / 输入方案页）。
#[cfg(any(windows, feature = "dict-page"))]
fn tab_bar<'a>(active: usize, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    let labels = ["用户词典", "快捷短语"];
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
            .on_press(Message::DictTab(i)),
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

// ---- 表格件（列头 / 斑马纹行 / 定宽单元格 / 行内按钮） ----

/// 定宽数据单元格：编码 / 频率 / 权重这类短内容列，定宽保证各行对齐。
#[cfg(any(windows, feature = "dict-page"))]
fn cell<'a>(value: String, width: f32, colors: &ThemeColors) -> Element<'a, Message> {
    text(value)
        .size(13)
        .font(medium())
        .color(colors.foreground)
        .width(Length::Fixed(width))
        .into()
}

/// 定宽列头单元格（与 `cell` 同宽，表头与数据行天然对齐）。
#[cfg(any(windows, feature = "dict-page"))]
fn head_cell<'a>(label: &'a str, width: f32, colors: &ThemeColors) -> Element<'a, Message> {
    text(label.to_string())
        .size(12)
        .font(medium())
        .color(colors.foreground_muted)
        .width(Length::Fixed(width))
        .into()
}

/// 表头行：弹性首列 + 若干定宽列 + 右对齐的操作列。
#[cfg(any(windows, feature = "dict-page"))]
fn table_header<'a>(
    first_label: &'a str,
    columns: &[(&'a str, f32)],
    action_width: f32,
    colors: &ThemeColors,
) -> Element<'a, Message> {
    let mut head = row![text(first_label.to_string())
        .size(12)
        .font(medium())
        .color(colors.foreground_muted)
        .width(Length::Fill)]
    .spacing(8)
    .align_y(Alignment::Center);
    for (label, width) in columns {
        head = head.push(head_cell(label, *width, colors));
    }
    head = head.push(
        container(
            text("操作".to_string())
                .size(12)
                .font(medium())
                .color(colors.foreground_muted),
        )
        .width(Length::Fixed(action_width))
        .align_x(iced::alignment::Horizontal::Right),
    );
    container(head).width(Length::Fill).padding([8, 14]).into()
}

/// 斑马纹数据行：偶数行淡底色，与卡片底色区分。
#[cfg(any(windows, feature = "dict-page"))]
fn zebra_row<'a>(
    content: Element<'a, Message>,
    index: usize,
    colors: &ThemeColors,
) -> Element<'a, Message> {
    let colors = *colors;
    let tinted = index % 2 == 1;
    container(content)
        .width(Length::Fill)
        .padding([6, 14])
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

/// 表格空态行（居中弱化文案）。
#[cfg(any(windows, feature = "dict-page"))]
fn table_empty<'a>(hint: String, colors: &ThemeColors) -> Element<'a, Message> {
    container(text(hint).size(13).color(colors.foreground_muted))
        .width(Length::Fill)
        .padding(28)
        .center_x(Length::Fill)
        .into()
}

/// 行内小按钮（普通操作：删除 / 取消 / 编辑）。
#[cfg(any(windows, feature = "dict-page"))]
fn row_button<'a>(
    label: &'static str,
    colors: &ThemeColors,
    on_press: Message,
) -> Element<'a, Message> {
    text_button(label, colors, on_press).padding([4, 10]).into()
}

/// 行内小按钮（危险操作：确认删除）。
#[cfg(any(windows, feature = "dict-page"))]
fn row_button_danger<'a>(
    label: &'static str,
    colors: &ThemeColors,
    on_press: Message,
) -> Element<'a, Message> {
    button_danger(label, colors, on_press)
        .padding([4, 10])
        .into()
}

/// 表格卡片：列头 + 数据行（空态）合成一张卡。
#[cfg(any(windows, feature = "dict-page"))]
fn table_card<'a>(
    header: Element<'a, Message>,
    body: Vec<Element<'a, Message>>,
    empty: Option<String>,
    colors: &ThemeColors,
) -> Element<'a, Message> {
    let colors = *colors;
    let mut table = column![].width(Length::Fill);
    table = table.push(header);
    if body.is_empty() {
        if let Some(hint) = empty {
            table = table.push(table_empty(hint, &colors));
        }
    } else {
        for row in body {
            table = table.push(row);
        }
    }
    container(table)
        .width(Length::Fill)
        .style(move |_| card_style(&colors))
        .into()
}

/// 单行弱化文案（状态 / 提示 / 目录信息）。
#[cfg(any(windows, feature = "dict-page"))]
fn hint_line<'a>(value: String, color: Color) -> Element<'a, Message> {
    text(value)
        .size(12)
        .font(medium())
        .color(color)
        .width(Length::Fill)
        .into()
}

// ---- Tab 0：用户词典 ----

/// 用户词典 Tab：词典下拉 + 搜索 + 词条表格 + 整本操作。
#[cfg(any(windows, feature = "dict-page"))]
fn user_dict_view<'a>(
    d: &crate::state::DictManageState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let browse = &d.browse;
    let mut items = column![].spacing(12).width(Length::Fill);

    // 工具行：词典选择 + 搜索 + 新增。
    let dict_options = d.dicts.clone();
    let current = if dict_options.contains(&browse.dict) {
        Some(browse.dict.clone())
    } else {
        None
    };
    let selector: Element<'a, Message> = if dict_options.is_empty() {
        container(
            text(if d.busy == Some("refresh") {
                "正在获取词典列表…".to_string()
            } else {
                "暂无用户词典".to_string()
            })
            .size(13)
            .color(colors.foreground_muted),
        )
        .padding([4, 2])
        .into()
    } else {
        pick_list(dict_options, current, Message::DictSelect)
            .placeholder("选择词典")
            .width(Length::Fixed(180.0))
            .into()
    };
    let add_button: Element<'a, Message> = if browse.writing {
        button_disabled("新增词条", colors)
    } else {
        button_primary("新增词条", colors, Message::DictEntryAddOpen).into()
    };
    items = items.push(
        row![
            selector,
            text_input("搜索词条或编码…", &browse.query)
                .on_input(Message::DictQueryChanged)
                .style(move |_t, s| text_input_style(colors, s))
                .width(Length::Fill),
            add_button,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );

    // 整本操作（作用于选中词典）：备份 / 恢复 / 导出 / 导入。
    // 放在表格**上方**：词条表每页 50 行，操作放表格下面等于埋掉。
    // 这四个操作和词条写入动的都是同一本用户词典（server 侧要引擎），
    // 任一在途（busy / writing）时全部禁用，不再"点了没反应"。
    if !browse.dict.is_empty() {
        let ops_locked = d.busy.is_some() || browse.writing;
        let bulk = [
            ("备份快照", Message::DictBackup(browse.dict.clone())),
            ("恢复快照…", Message::DictRestore),
            ("导出文本…", Message::DictExport(browse.dict.clone())),
            ("导入文本…", Message::DictImport(browse.dict.clone())),
        ];
        let mut ops = row![].spacing(8).align_y(Alignment::Center);
        for (label, message) in bulk {
            let cell: Element<'a, Message> = if ops_locked {
                button_disabled(label, colors)
            } else {
                text_button(label, colors, message).into()
            };
            ops = ops.push(cell);
        }
        items = items.push(ops);
    }

    // 操作结果消息（备份/恢复/导出/导入/列表失败）：贴着操作行，别沉到表格下面。
    if let Some(m) = &d.message {
        items = items.push(hint_line(m.clone(), colors.foreground));
    }

    // 写入结果提示（主色一行；错误走页脚状态行的 error 色）。
    if let Some(notice) = &browse.notice {
        items = items.push(hint_line(notice.clone(), colors.primary));
    }

    // 词条表：词（弹性）+ 编码 + 频率 + 操作。
    let header = table_header("词", &[("编码", 110.0), ("频率", 70.0)], 180.0, colors);
    let mut body: Vec<Element<'a, Message>> = Vec::new();
    for (index, entry) in browse.page_entries().iter().enumerate() {
        let actions: Element<'a, Message> = if browse.writing {
            // 写入在途：按钮一律禁用（避免并发改词库）。
            button_disabled("删除", colors)
        } else if browse
            .delete_confirm
            .as_ref()
            .is_some_and(|(word, code)| word == &entry.word && code == &entry.code)
        {
            let mut confirm = row![].spacing(8);
            confirm = confirm.push(row_button("取消", colors, Message::DictEntryDeleteCancel));
            confirm = confirm.push(row_button_danger(
                "确认删除",
                colors,
                Message::DictEntryDeleteConfirm(entry.word.clone(), entry.code.clone()),
            ));
            confirm.into()
        } else {
            row_button(
                "删除",
                colors,
                Message::DictEntryDeleteRequest(entry.word.clone(), entry.code.clone()),
            )
        };
        let mut line = row![
            text(entry.word.clone())
                .size(13)
                .font(medium())
                .color(colors.foreground)
                .width(Length::Fill),
            cell(entry.code.clone(), 110.0, colors),
            cell(entry.commits.to_string(), 70.0, colors),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        line = line.push(
            container(actions)
                .width(Length::Fixed(180.0))
                .align_x(iced::alignment::Horizontal::Right),
        );
        body.push(zebra_row(line.into(), index, colors));
    }
    let empty = if browse.dict.is_empty() {
        if d.busy.is_some() {
            None
        } else {
            Some("暂无用户词典：打字造词后 rime 会自动生成，点「刷新」重新获取".to_string())
        }
    } else if browse.loading {
        Some("正在读取词条…".to_string())
    } else if browse.error.is_some() {
        None
    } else if browse.entries.is_empty() && browse.loaded {
        if browse.query.trim().is_empty() {
            Some("这本词典还没有词条；打字造词自动积累，或点「新增词条」手动添加".to_string())
        } else {
            Some("没有匹配的词条，换个关键词试试".to_string())
        }
    } else {
        None
    };
    items = items.push(table_card(header, body, empty, colors));

    // 页脚：状态统计居左，翻页居右。
    let status_color = if browse.error.is_some() {
        colors.error
    } else {
        colors.foreground_muted
    };
    let mut footer = row![text(browse.status_text())
        .size(12)
        .font(medium())
        .color(status_color)]
    .spacing(8)
    .align_y(Alignment::Center);
    let pages = browse.page_count();
    if pages > 1 {
        footer = footer.push(Space::new().width(Length::Fill));
        footer = footer.push(
            text(format!(
                "第 {} / {} 页 · 每页 {DICT_ENTRIES_PAGE_SIZE} 条",
                browse.page + 1,
                pages
            ))
            .size(12)
            .font(medium())
            .color(colors.foreground_muted),
        );
        footer = footer.push(row_button(
            "上一页",
            colors,
            Message::DictEntriesPage(browse.page.saturating_sub(1)),
        ));
        footer = footer.push(row_button(
            "下一页",
            colors,
            Message::DictEntriesPage(browse.page.saturating_add(1)),
        ));
    }
    items = items.push(footer);

    // 页尾参考信息：整本操作的语义说明 + 快照目录（静态信息沉底）。
    if !browse.dict.is_empty() {
        items = items.push(hint_line(
            "备份快照存到同步目录；恢复从快照合并；导出/导入是纯文本（可跨设备分享）".to_string(),
            colors.foreground_muted,
        ));
    }
    if !d.sync_dir.is_empty() {
        items = items.push(hint_line(
            format!("快照目录：{}", d.sync_dir),
            colors.foreground_muted,
        ));
    }

    items.into()
}

// ---- Tab 1：快捷短语 ----

/// 方案下拉选项：id 参与相等性比较，避免同名方案选中错乱。
#[cfg(any(windows, feature = "dict-page"))]
#[derive(Clone, PartialEq)]
struct SchemaOption {
    id: String,
    name: String,
}

#[cfg(any(windows, feature = "dict-page"))]
impl std::fmt::Display for SchemaOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// 快捷短语 Tab：方案下拉 + 短语表格 + 部署。
#[cfg(any(windows, feature = "dict-page"))]
fn phrase_view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let Some(phrase) = settings.dict_manage.phrase.as_ref() else {
        // 还没有可用方案：进 Tab 时自动打开，这里只剩「无方案」的空态。
        return column![container(
            text("还没有可用的输入方案；安装方案后可在这里维护快捷短语")
                .size(13)
                .color(colors.foreground_muted),
        )
        .width(Length::Fill)
        .padding(28)
        .center_x(Length::Fill)]
        .width(Length::Fill)
        .into();
    };

    let mut items = column![].spacing(12).width(Length::Fill);

    // 工具行：方案选择（居左）+ 新增短语（居右）。
    let options: Vec<SchemaOption> = settings
        .input_schema
        .available_schemas
        .iter()
        .map(|info| SchemaOption {
            id: info.schema_id.clone(),
            name: info.name.clone(),
        })
        .collect();
    let current = options
        .iter()
        .find(|option| option.id == phrase.schema_id)
        .cloned();
    let mut toolbar = row![].spacing(8).align_y(Alignment::Center);
    if !options.is_empty() {
        toolbar = toolbar.push(
            pick_list(options, current, |chosen| {
                Message::DictPhraseSchemaChanged(chosen.id)
            })
            .placeholder("选择方案")
            .width(Length::Fixed(180.0)),
        );
    }
    toolbar = toolbar.push(Space::new().width(Length::Fill));
    if phrase.saving || phrase.loading {
        toolbar = toolbar.push(button_disabled("新增短语", colors));
    } else {
        toolbar = toolbar.push(button_primary(
            "新增短语",
            colors,
            Message::DictPhraseAddOpen,
        ));
    }
    items = items.push(toolbar);

    // 表文件信息行。
    let file_text = if !phrase.loaded {
        "短语表：尚未读取".to_string()
    } else {
        format!(
            "短语表 {}（{}）· 翻译器已启用：{}",
            phrase.dict_name,
            phrase.file_name,
            if phrase.patch_applied { "是" } else { "否" }
        )
    };
    items = items.push(hint_line(file_text, colors.foreground_muted));

    // 状态行（加载/保存/错误/提示）。
    if let Some(status) = phrase_status_text(phrase) {
        let color = if phrase.error.is_some() {
            colors.error
        } else if phrase.notice.is_some() {
            colors.primary
        } else {
            colors.foreground_muted
        };
        items = items.push(hint_line(status, color));
    }

    // 短语表：短语（弹性）+ 编码 + 权重 + 操作。
    let header = table_header("短语", &[("编码", 110.0), ("权重", 70.0)], 180.0, colors);
    let busy = phrase.saving || phrase.loading;
    let mut body: Vec<Element<'a, Message>> = Vec::new();
    for (index, entry) in phrase.entries.iter().enumerate() {
        let actions: Element<'a, Message> = if busy {
            row![].into()
        } else if phrase.delete_confirm == Some(index) {
            let mut confirm = row![].spacing(8);
            confirm = confirm.push(row_button("取消", colors, Message::DictPhraseDeleteCancel));
            confirm = confirm.push(row_button_danger(
                "确认删除",
                colors,
                Message::DictPhraseDeleteConfirm(index),
            ));
            confirm.into()
        } else {
            let mut actions = row![].spacing(8);
            actions = actions.push(row_button("编辑", colors, Message::DictPhraseEdit(index)));
            actions = actions.push(row_button(
                "删除",
                colors,
                Message::DictPhraseDeleteRequest(index),
            ));
            actions.into()
        };
        let weight = match entry.weight {
            Some(weight) => weight.to_string(),
            None => "—".to_string(),
        };
        let mut line = row![
            text(entry.word.clone())
                .size(13)
                .font(medium())
                .color(colors.foreground)
                .width(Length::Fill),
            cell(entry.code.clone(), 110.0, colors),
            cell(weight, 70.0, colors),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        line = line.push(
            container(actions)
                .width(Length::Fixed(180.0))
                .align_x(iced::alignment::Horizontal::Right),
        );
        body.push(zebra_row(line.into(), index, colors));
    }
    let empty = if busy {
        Some(if phrase.loading {
            "正在读取短语表…".to_string()
        } else {
            "正在保存短语表…".to_string()
        })
    } else if phrase.error.is_some() {
        None
    } else if phrase.entries.is_empty() && phrase.loaded {
        Some("还没有快捷短语；点「新增短语」加一条，比如编码 lh → 短语「你好」".to_string())
    } else {
        None
    };
    items = items.push(table_card(header, body, empty, colors));

    // 页脚：生效条件居左，部署按钮居右（不自动部署的显式入口）。
    let deploy_hint = if phrase.patch_added {
        "已启用快捷短语翻译器；部署方案后短语才会出现在候选栏"
    } else {
        "短语表内容与翻译器的改动都要在重新部署方案后生效"
    };
    items = items.push(
        row![
            text(deploy_hint.to_string())
                .size(12)
                .font(medium())
                .color(colors.foreground_muted)
                .width(Length::Fill),
            button_primary("部署方案", colors, Message::DeploySchemas),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );

    items.into()
}

/// 短语 Tab 的状态文案（None = 无事发生，不渲染空行）。
#[cfg(any(windows, feature = "dict-page"))]
fn phrase_status_text(phrase: &CustomPhraseState) -> Option<String> {
    if phrase.loading {
        Some("正在读取短语表…".to_string())
    } else if phrase.saving {
        Some("正在保存短语表…".to_string())
    } else if let Some(error) = &phrase.error {
        Some(error.clone())
    } else {
        phrase.notice.clone()
    }
}

// ---- 模态对话框 ----

/// 新增词条对话框：词 / 编码 / 频率。
#[cfg(any(windows, feature = "dict-page"))]
fn entry_add_dialog<'a>(
    draft: &'a crate::state::DictEntryDraft,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    column![
        text("新增词条")
            .size(16)
            .font(semibold())
            .color(colors.foreground),
        text("词（上屏的文字）")
            .size(13)
            .color(colors.foreground_muted),
        text_input("如：曦码", &draft.word)
            .on_input(Message::DictEntryAddWordChanged)
            .style(move |_t, s| text_input_style(colors, s))
            .width(Length::Fill),
        text("编码").size(13).color(colors.foreground_muted),
        text_input("如：jhdm", &draft.code)
            .on_input(Message::DictEntryAddCodeChanged)
            .style(move |_t, s| text_input_style(colors, s))
            .width(Length::Fill),
        text("频率（正整数，留空即 1）")
            .size(13)
            .color(colors.foreground_muted),
        text_input("如：1", &draft.commits)
            .on_input(Message::DictEntryAddCommitsChanged)
            .style(move |_t, s| text_input_style(colors, s))
            .width(Length::Fill),
        text("写入要短暂重启输入会话，会打断正在输入的句子")
            .size(12)
            .color(colors.foreground_muted),
        row![
            crate::components::widgets::button_secondary(
                "取消",
                colors,
                Message::DictEntryAddCancel
            ),
            button_primary("添加", colors, Message::DictEntryAddSubmit),
        ]
        .spacing(8),
    ]
    .spacing(8)
    .into()
}

/// 短语新增/编辑对话框：词 / 编码 / 权重。
#[cfg(any(windows, feature = "dict-page"))]
fn phrase_dialog_view<'a>(
    dialog: &'a crate::state::PhraseDialogState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let title = if dialog.editing.is_some() {
        "编辑短语"
    } else {
        "新增短语"
    };
    let submit = if dialog.editing.is_some() {
        "保存"
    } else {
        "添加"
    };
    column![
        text(title)
            .size(16)
            .font(semibold())
            .color(colors.foreground),
        text("短语（上屏的文字）")
            .size(13)
            .color(colors.foreground_muted),
        text_input("如：会议纪要见群公告", &dialog.word)
            .on_input(Message::DictPhraseDialogWordChanged)
            .style(move |_t, s| text_input_style(colors, s))
            .width(Length::Fill),
        text("编码（输入这串后短语进候选栏）")
            .size(13)
            .color(colors.foreground_muted),
        text_input("如：hy", &dialog.code)
            .on_input(Message::DictPhraseDialogCodeChanged)
            .style(move |_t, s| text_input_style(colors, s))
            .width(Length::Fill),
        text("权重（正整数，越大越靠前；留空走默认）")
            .size(13)
            .color(colors.foreground_muted),
        text_input("如：99", &dialog.weight)
            .on_input(Message::DictPhraseDialogWeightChanged)
            .style(move |_t, s| text_input_style(colors, s))
            .width(Length::Fill),
        row![
            crate::components::widgets::button_secondary(
                "取消",
                colors,
                Message::DictPhraseDialogCancel,
            ),
            button_primary(submit, colors, Message::DictPhraseDialogSubmit),
        ]
        .spacing(8),
    ]
    .spacing(8)
    .into()
}

/// 非 Windows 且未启用 dict-page：占位（rime 操作由宿主经回调注入）。
#[cfg(not(any(windows, feature = "dict-page")))]
pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    use crate::components::settings::{settings_group, settings_item, settings_page};
    use crate::components::widgets::label;

    settings_page(
        "词典管理",
        colors,
        vec![settings_group(
            "用户词典",
            Some("管理用户词库"),
            colors,
            vec![settings_item(
                "用户词典",
                Some("用户词典由 Rime 引擎自动维护"),
                colors,
                label("Rime 自动管理", colors),
            )],
        )],
    )
}
