#![cfg(feature = "backup-page")]
use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{
    button_primary, button_secondary, label, text_button, text_input_style,
};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{column, container, pick_list, row, text, text_input};
use iced::{Element, Length};

const MODES: [&str; 2] = ["仅配置", "全量"];

/// 云备份页：WebDAV 连接 + 备份模式 + 备份/恢复操作（对齐 Android 云备份页）。
pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let b = &settings.backup;
    let editable = b.busy.is_none();

    // WebDAV 连接配置。
    let url_input = text_input("https://dav.example.com/dav", &b.url)
        .on_input_maybe(editable.then_some(Message::BackupUrlChanged))
        .style(move |_t, s| text_input_style(colors, s))
        .width(Length::FillPortion(2));
    let user_input = text_input("用户名", &b.username)
        .on_input_maybe(editable.then_some(Message::BackupUsernameChanged))
        .style(move |_t, s| text_input_style(colors, s))
        .width(Length::FillPortion(2));
    let pass_input = text_input("", &b.password)
        .on_input_maybe(editable.then_some(Message::BackupPasswordChanged))
        .secure(true)
        .style(move |_t, s| text_input_style(colors, s))
        .width(Length::FillPortion(2));
    let dir_input = text_input("xime-backup", &b.remote_dir)
        .on_input_maybe(editable.then_some(Message::BackupDirChanged))
        .style(move |_t, s| text_input_style(colors, s))
        .width(Length::FillPortion(2));
    let test_control: Element<'a, Message> = if b.busy == Some("test") {
        label("测试中…", colors)
    } else {
        text_button("测试连接", colors, Message::BackupTest).into()
    };

    // 备份模式选择（pick_list 只读当前模式标签，回写索引）。
    let current_mode = MODES.get(b.mode as usize).copied().unwrap_or(MODES[0]);
    let mode_pick = pick_list(MODES, Some(current_mode), |chosen| {
        Message::BackupModeChanged(MODES.iter().position(|m| *m == chosen).unwrap_or(0) as u8)
    });

    // 备份操作行：忙碌时显示进度文案。
    let op_control: Element<'a, Message> = match b.busy {
        Some("backup") => label("正在备份…", colors),
        Some("list") => label("正在获取远端列表…", colors),
        Some("restore") => label("正在恢复…", colors),
        Some("delete") => label("正在删除…", colors),
        Some(_) => label("正在连接…", colors),
        None => row![
            button_primary("立即备份", colors, Message::BackupNow),
            button_secondary("查看远端备份", colors, Message::BackupList),
        ]
        .spacing(8)
        .into(),
    };

    let mut groups = vec![
        settings_group(
            "备份服务",
            Some("通过 WebDAV 云端存储备份输入法配置与词库"),
            colors,
            vec![
                settings_item(
                    "服务器地址",
                    Some("WebDAV 根地址（含路径）"),
                    colors,
                    url_input.into(),
                ),
                settings_item("用户名", None::<String>, colors, user_input.into()),
                settings_item(
                    "密码",
                    Some("明文保存于本地配置文件 (0600)"),
                    colors,
                    pass_input.into(),
                ),
                settings_item(
                    "远端目录",
                    Some("备份包存放的目录名"),
                    colors,
                    dir_input.into(),
                ),
                settings_item("连接", None::<String>, colors, test_control),
            ],
        ),
        settings_group(
            "备份设置",
            None::<String>,
            colors,
            vec![settings_item(
                "备份模式",
                Some(
                    crate::backup::BackupMode::from_index(b.mode)
                        .unwrap_or(crate::backup::BackupMode::ConfigOnly)
                        .detail(),
                ),
                colors,
                mode_pick.into(),
            )],
        ),
        settings_group(
            "备份操作",
            Some("备份内容为 rime 用户目录（方案配置、自造词等）"),
            colors,
            vec![
                settings_item("操作", b.message.as_deref(), colors, op_control),
                settings_item(
                    "说明",
                    Some("恢复覆盖 rime 目录同名文件，重启输入法后生效"),
                    colors,
                    label("", colors),
                ),
            ],
        ),
    ];

    // 远端备份列表。
    let mut remote_items: Vec<Element<'a, Message>> = Vec::new();
    if b.remote.is_empty() {
        remote_items.push(
            container(text("远端暂无备份").size(14).color(colors.foreground_muted))
                .width(Length::Fill)
                .into(),
        );
    } else {
        for file in &b.remote {
            let mut sub = String::new();
            if let Some(size) = file.size {
                sub.push_str(&format_size(size));
            }
            if let Some(tag) = crate::backup::mode_tag_of(&file.name) {
                if !sub.is_empty() {
                    sub.push_str(" · ");
                }
                sub.push_str(if tag == "full" { "全量" } else { "仅配置" });
            }
            let left = column![
                text(&file.name).size(14).color(colors.foreground),
                text(sub).size(12).color(colors.foreground_muted),
            ]
            .spacing(2)
            .width(Length::Fill);
            let actions = row![
                text_button("恢复", colors, Message::BackupRestore(file.path.clone())),
                text_button("删除", colors, Message::BackupDelete(file.path.clone())),
            ]
            .spacing(4);
            remote_items.push(row![left, actions].spacing(8).width(Length::Fill).into());
        }
    }
    groups.push(settings_group(
        "远端备份",
        None::<String>,
        colors,
        remote_items,
    ));

    settings_page("云备份", colors, groups)
}

/// 字节 → 人类可读大小（KB / MB，与 Android 展示一致）。
fn format_size(size: u64) -> String {
    let mb = size as f64 / 1024.0 / 1024.0;
    if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.1} KB", size as f64 / 1024.0)
    }
}
