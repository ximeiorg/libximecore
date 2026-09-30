//! 词典管理页：用户词典列表 + 备份/恢复/导出/导入
//! （对齐 weasel DictManagementDialog；rime 操作经 IPC 在 server 执行）。
use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::label;
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::Element;

#[cfg(windows)]
use crate::components::widgets::text_button;
#[cfg(windows)]
use iced::widget::{row, text};

/// Windows：真实词典管理（rime 用户词典经 IPC 操作）。
#[cfg(windows)]
pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let d = &settings.dict_manage;
    let mut items: Vec<Element<'a, Message>> = Vec::new();

    // 操作工具行（表头）。
    items.push(settings_item(
        "操作",
        Some("备份：词库快照存到同步目录；恢复：从快照文件合并；导出/导入：纯文本（可跨设备分享）"),
        colors,
        row![
            text_button("刷新", colors, Message::DictRefresh),
            text_button("恢复快照…", colors, Message::DictRestore),
        ]
        .spacing(8)
        .into(),
    ));

    items.push(settings_item(
        "快照目录",
        Some("用户词典快照（*.userdb.txt）的存放位置，也是「用户资料同步」的快照目录"),
        colors,
        text(if d.sync_dir.is_empty() {
            "（未知，点「刷新」获取）".to_string()
        } else {
            d.sync_dir.clone()
        })
        .size(13)
        .color(colors.foreground_muted)
        .into(),
    ));

    if d.busy == Some("refresh") {
        items.push(settings_item(
            "正在获取词典列表…",
            None::<String>,
            colors,
            label("", colors),
        ));
    }

    for dict in &d.dicts {
        items.push(settings_item(
            dict.clone(),
            None::<String>,
            colors,
            row![
                text_button("备份", colors, Message::DictBackup(dict.clone())),
                text_button("导出", colors, Message::DictExport(dict.clone())),
                text_button("导入", colors, Message::DictImport(dict.clone())),
            ]
            .spacing(8)
            .into(),
        ));
    }

    if d.dicts.is_empty() && d.busy.is_none() {
        items.push(settings_item(
            "暂无用户词典",
            Some("打字造词后 rime 会自动生成用户词典；点「刷新」重新获取"),
            colors,
            label("", colors),
        ));
    }

    if let Some(m) = &d.message {
        items.push(settings_item(
            "状态",
            None::<String>,
            colors,
            text(m.clone())
                .size(13)
                .color(colors.foreground_muted)
                .into(),
        ));
    }

    settings_page(
        "词典管理",
        colors,
        vec![settings_group(
            "用户词典",
            Some("用户词典记录打字造词，由 rime 自动维护；备份的快照与「用户资料同步」共用目录"),
            colors,
            items,
        )],
    )
}

/// 非 Windows：占位（rime 操作当前经 server/IPC 承载）。
#[cfg(not(windows))]
pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
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
