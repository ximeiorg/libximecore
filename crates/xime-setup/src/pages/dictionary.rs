//! 词典管理页：入口页 + 三个子页面（用户词库 / 快捷短语 / 导入词库）。
//!
//! 版式对齐 Android 的「词库管理」：入口页只列三行，各自点进独立子页面。
//! 以前这里是一排页签，等于按「数据落在哪个文件里」分类——用户得先认识
//! `.userdb`、`custom_phrase`、`*.dict.yaml`，才知道自己要找的东西在哪个签里。
use crate::components::settings::settings_group;
use crate::components::widgets::{medium, semibold, text_button_muted};
#[cfg(any(windows, feature = "dict-page"))]
use crate::components::widgets::{button_secondary, text_button, text_button_danger};
use crate::state::{DictSub, Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Background, Element, Length};

#[cfg(any(windows, feature = "dict-page"))]
use crate::components::widgets::{
    button_danger, button_disabled, button_primary, card_style, pick_list_style, text_input_style,
};
#[cfg(any(windows, feature = "dict-page"))]
use crate::state::{CustomPhraseState, DICT_ENTRIES_PAGE_SIZE};
#[cfg(any(windows, feature = "dict-page"))]
use iced::widget::{pick_list, text_input};
#[cfg(any(windows, feature = "dict-page"))]
use iced::Color;

/// 词典页：入口页或子页面（子页面顶部带「返回词库管理」）。
pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let sub = settings.dict_sub;

    let mut header = row![].spacing(14).align_y(Alignment::Center);
    if sub != DictSub::Hub {
        header = header.push(text_button_muted("返回", colors, Message::DictSubBack));
    }
    header = header.push(
        text(dict_sub_title(sub))
            .size(20)
            .font(semibold())
            .color(colors.foreground),
    );
    header = header.push(Space::new().width(Length::Fill));
    if let Some(action) = header_action(settings, sub, colors) {
        header = header.push(action);
    }

    // 弹窗不在这里叠：页面在 scrollable 里，只有内容高度，叠上来会被裁。
    // 由 `pages::page_overlay` 交给 app 层，压在整窗上。
    column![header, body_view(settings, sub, colors)]
        .spacing(16)
        .padding(20)
        .width(Length::Fill)
        .into()
}

fn dict_sub_title(sub: DictSub) -> &'static str {
    match sub {
        DictSub::Hub => "词库管理",
        DictSub::UserDict => "用户词库",
        DictSub::Phrases => "快捷短语",
        DictSub::Import => "导入词库",
    }
}

fn body_view<'a>(
    settings: &'a SettingsState,
    sub: DictSub,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    match sub {
        DictSub::Hub => hub_view(settings, colors),
        DictSub::Import => crate::pages::dict_import::view(settings, colors),
        #[cfg(any(windows, feature = "dict-page"))]
        DictSub::UserDict => user_dict_view(&settings.dict_manage, colors),
        #[cfg(any(windows, feature = "dict-page"))]
        DictSub::Phrases => phrase_view(settings, colors),
        // 没有 dict-page 的宿主：这两行入口不可点，兜底回入口页。
        #[cfg(not(any(windows, feature = "dict-page")))]
        _ => hub_view(settings, colors),
    }
}

/// 页头右侧操作（只有子页面有：各自的主操作 / 刷新）。
fn header_action<'a>(
    settings: &'a SettingsState,
    sub: DictSub,
    colors: &'a ThemeColors,
) -> Option<Element<'a, Message>> {
    match sub {
        DictSub::Import => Some(crate::pages::dict_import::header_action(
            &settings.dict_import,
            colors,
        )),
        #[cfg(any(windows, feature = "dict-page"))]
        DictSub::UserDict => Some(text_button_muted("刷新", colors, Message::DictRefresh).into()),
        _ => None,
    }
}

/// 同一时刻只会有一个弹窗（新增词条 / 短语编辑 / 导入）。
pub fn overlay<'a>(
    settings: &'a SettingsState,
    colors: &'a ThemeColors,
) -> Option<Element<'a, Message>> {
    #[cfg(any(windows, feature = "dict-page"))]
    let inner = settings
        .dict_manage
        .browse
        .add_dialog
        .as_ref()
        .map(|draft| entry_add_dialog(draft, colors))
        .or_else(|| {
            settings
                .dict_manage
                .phrase
                .as_ref()
                .and_then(|phrase| phrase.dialog.as_ref())
                .map(|dialog| phrase_dialog_view(dialog, colors))
        });
    #[cfg(not(any(windows, feature = "dict-page")))]
    let inner: Option<Element<'a, Message>> = None;

    inner.or_else(|| {
        settings
            .dict_import
            .dialog
            .then(|| crate::pages::dict_import::dialog(settings, colors))
    })
}

// ---- 入口页（对齐 Android「词库管理」的三行入口） ----

fn hub_view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let mut rows: Vec<Element<'a, Message>> = Vec::new();

    #[cfg(any(windows, feature = "dict-page"))]
    let (user_sub, phrase_sub, user_ready) = {
        let d = &settings.dict_manage;
        let user = if d.dicts.is_empty() {
            "输入时自动积累的词条".to_string()
        } else {
            format!("输入时自动积累的词条 · {} 本词典", d.dicts.len())
        };
        let phrase = match d.phrase.as_ref() {
            Some(phrase) if !phrase.entries.is_empty() => {
                format!("补充方案码表之外的常用短语 · {} 条", phrase.entries.len())
            }
            _ => "补充方案码表之外的常用短语".to_string(),
        };
        (user, phrase, true)
    };
    #[cfg(not(any(windows, feature = "dict-page")))]
    let (user_sub, phrase_sub, user_ready) = (
        "由输入法维护的词条（当前宿主未接入）".to_string(),
        "由输入法维护的自定义短语（当前宿主未接入）".to_string(),
        false,
    );

    rows.push(hub_row(
        "用户词库",
        user_sub,
        user_ready,
        Message::DictSubOpen(DictSub::UserDict),
        colors,
    ));
    rows.push(hub_divider(colors));
    rows.push(hub_row(
        "快捷短语",
        phrase_sub,
        user_ready,
        Message::DictSubOpen(DictSub::Phrases),
        colors,
    ));
    rows.push(hub_divider(colors));

    let import_sub = if settings.dict_import.items.is_empty() {
        "把其他输入法的词库转换成 Rime 词典".to_string()
    } else {
        format!(
            "把其他输入法的词库转换成 Rime 词典 · 已导入 {} 本",
            settings.dict_import.items.len()
        )
    };
    rows.push(hub_row(
        "导入词库",
        import_sub,
        true,
        Message::DictSubOpen(DictSub::Import),
        colors,
    ));

    column![
        settings_group(
            "词库内容",
            Some("三类内容分开维护：用户词库随输入自动长大，快捷短语手写，导入词库由外部文件生成。"),
            colors,
            rows,
        ),
        footnote(colors),
    ]
    .spacing(16)
    .width(Length::Fill)
    .into()
}

/// 入口行：标题 + 副标题 + 右箭头，整行可点（hover 变底色）。
fn hub_row<'a>(
    title: &'a str,
    subtitle: String,
    enabled: bool,
    on_press: Message,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let palette = *colors;
    let body = row![
        column![
            text(title).size(14).font(medium()).color(if enabled {
                palette.foreground
            } else {
                palette.foreground_faint
            }),
            text(subtitle).size(12).color(palette.foreground_muted),
        ]
        .spacing(3)
        .width(Length::Fill),
        text("›")
            .size(16)
            .color(if enabled {
                palette.foreground_muted
            } else {
                palette.foreground_faint
            }),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .padding([10, 6])
    .width(Length::Fill);

    if !enabled {
        return body.into();
    }

    button(body)
        .on_press(on_press)
        .padding(0)
        .width(Length::Fill)
        .style(move |_theme, status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: hovered.then_some(Background::Color(palette.surface_hover)),
                text_color: palette.foreground,
                border: iced::border::Border {
                    radius: 8.0.into(),
                    ..Default::default()
                },
                ..button::Style::default()
            }
        })
        .into()
}

/// 入口行之间的细线（与卡片内容同宽）。
fn hub_divider<'a>(colors: &'a ThemeColors) -> Element<'a, Message> {
    let palette = *colors;
    container(Space::new().width(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(palette.border)),
            ..container::Style::default()
        })
        .into()
}

/// 入口页脚注：指路到方案词表（静态词表属于方案本身）。
fn footnote(colors: &ThemeColors) -> Element<'_, Message> {
    text("方案自带的静态词表不在词库里，在「输入方案 → 方案词表」中查看。".to_string())
        .size(12)
        .color(colors.foreground_muted)
        .width(Length::Fill)
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
    container(
        text(label.to_string())
            .size(12)
            .font(medium())
            .color(colors.foreground_muted),
    )
    .width(Length::Fixed(width))
    .align_x(iced::alignment::Horizontal::Right)
    .into()
}

/// 右对齐的数据单元格（数字列用：频率、权重这类右对齐才好纵向比较）。
#[cfg(any(windows, feature = "dict-page"))]
fn cell_right<'a>(value: String, width: f32, colors: &ThemeColors) -> Element<'a, Message> {
    container(
        text(value)
            .size(13)
            .color(colors.foreground),
    )
    .width(Length::Fixed(width))
    .align_x(iced::alignment::Horizontal::Right)
    .into()
}

/// 表头行：弹性首列 + 若干定宽列 + 右对齐的操作列。
#[cfg(any(windows, feature = "dict-page"))]
fn table_header<'a>(
    first_label: &'a str,
    first_width: Option<f32>,
    columns: &[(&'a str, f32)],
    action_width: f32,
    colors: &ThemeColors,
) -> Element<'a, Message> {
    let mut head = row![text(first_label.to_string())
        .size(12)
        .font(medium())
        .color(colors.foreground_muted)
        .width(match first_width {
            Some(width) => Length::Fixed(width),
            None => Length::Fill,
        })]
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
    let palette = *colors;
    column![
        container(head)
            .width(Length::Fill)
            .padding([0, 14])
            .height(Length::Fixed(34.0))
            .align_y(iced::alignment::Vertical::Center),
        container(Space::new().width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fixed(1.0))
            .style(move |_| container::Style {
                background: Some(Background::Color(palette.border)),
                ..container::Style::default()
            }),
    ]
    .width(Length::Fill)
    .into()
}

/// 数据行：定高 + 底部 1px 分隔线。
///
/// 原来用斑马纹（隔行浅灰底）：卡片本身就是浅色面，两种浅色叠在一起只剩
/// 「花」，行与行的关系反而不清楚。改分隔线后视线不被色块打断。
#[cfg(any(windows, feature = "dict-page"))]
fn zebra_row<'a>(
    content: Element<'a, Message>,
    _index: usize,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let colors = *colors;
    column![
        container(content)
            .width(Length::Fill)
            .height(Length::Fixed(36.0))
            .padding([0, 14])
            .align_y(iced::alignment::Vertical::Center),
        container(Space::new().width(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fixed(1.0))
            .style(move |_| container::Style {
                background: Some(Background::Color(colors.border)),
                ..container::Style::default()
            }),
    ]
    .width(Length::Fill)
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
    d: &'a crate::state::DictManageState,
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
        pick_list(dict_options, current, Message::DictSelect).text_size(14).padding([8, 10])
            .style(move |_theme, status| pick_list_style(colors, status))
        .menu_style(crate::components::widgets::pick_list_menu_style(colors))
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
            crate::components::widgets::search_input(
                "搜索词条或编码",
                &browse.query,
                Message::DictQueryChanged,
                colors,
            ),
            add_button,
        ]
        .spacing(10)
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
                button_secondary(label, colors, message).into()
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
    let header = table_header("词", None, &[("编码", 150.0), ("频率", 80.0)], 120.0, colors);
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
            text_button_danger(
                "删除",
                colors,
                Message::DictEntryDeleteRequest(entry.word.clone(), entry.code.clone()),
            )
            .into()
        };
        let line = row![
            text(entry.word.clone())
                .size(13)
                .font(medium())
                .color(colors.foreground)
                .width(Length::Fill),
            cell(entry.code.clone(), 150.0, colors),
            cell_right(entry.commits.to_string(), 80.0, colors),
            container(actions)
                .width(Length::Fixed(120.0))
                .align_x(iced::alignment::Horizontal::Right),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
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
        // 失败时别只留一行表头：表格里给一句「没读到」，具体原因在状态行。
        Some("没有读到词条".to_string())
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
            }).text_size(14).padding([8, 10])
            .placeholder("选择方案")
            .width(Length::Fixed(180.0))
            .style(move |_theme, status| pick_list_style(colors, status))
        .menu_style(crate::components::widgets::pick_list_menu_style(colors)),
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
    let header = table_header("短语", None, &[("编码", 150.0), ("权重", 80.0)], 120.0, colors);
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
            actions = actions.push(text_button_muted(
                "编辑",
                colors,
                Message::DictPhraseEdit(index),
            ));
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
        let line = row![
            text(entry.word.clone())
                .size(13)
                .font(medium())
                .color(colors.foreground)
                .width(Length::Fill),
            cell(entry.code.clone(), 150.0, colors),
            cell_right(weight, 80.0, colors),
            container(actions)
                .width(Length::Fixed(120.0))
                .align_x(iced::alignment::Horizontal::Right),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
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
        text_input("如：曦码", &draft.word).size(14).padding([8, 10])
            .on_input(Message::DictEntryAddWordChanged)
            .style(move |_t, s| text_input_style(colors, s))
            .width(Length::Fill),
        text("编码").size(13).color(colors.foreground_muted),
        text_input("如：jhdm", &draft.code).size(14).padding([8, 10])
            .on_input(Message::DictEntryAddCodeChanged)
            .style(move |_t, s| text_input_style(colors, s))
            .width(Length::Fill),
        text("频率（正整数，留空即 1）")
            .size(13)
            .color(colors.foreground_muted),
        text_input("如：1", &draft.commits).size(14).padding([8, 10])
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
        text_input("如：会议纪要见群公告", &dialog.word).size(14).padding([8, 10])
            .on_input(Message::DictPhraseDialogWordChanged)
            .style(move |_t, s| text_input_style(colors, s))
            .width(Length::Fill),
        text("编码（输入这串后短语进候选栏）")
            .size(13)
            .color(colors.foreground_muted),
        text_input("如：hy", &dialog.code).size(14).padding([8, 10])
            .on_input(Message::DictPhraseDialogCodeChanged)
            .style(move |_t, s| text_input_style(colors, s))
            .width(Length::Fill),
        text("权重（正整数，越大越靠前；留空走默认）")
            .size(13)
            .color(colors.foreground_muted),
        text_input("如：99", &dialog.weight).size(14).padding([8, 10])
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
