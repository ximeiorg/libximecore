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

/// 云备份页：提供者选择（内置 WebDAV / backup 插件）+ 备份模式 + 操作
/// （对齐 Android 云备份页：备份服务 / 备份设置 / 备份操作 / 远端备份）。
pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let b = &settings.backup;
    let editable = b.busy.is_none();

    // 提供者 pick_list。
    let provider_labels: Vec<&str> = b.providers.iter().map(|p| p.label()).collect();
    let current_provider = provider_labels
        .get(b.provider)
        .copied()
        .unwrap_or("内置 WebDAV");
    let provider_pick = pick_list(provider_labels, Some(current_provider), |chosen| {
        Message::BackupProviderChanged(
            b.providers
                .iter()
                .position(|p| p.label() == chosen)
                .unwrap_or(0),
        )
    });

    // 提供者相关配置项。
    let mut service_items: Vec<Element<'a, Message>> = vec![settings_item(
        "提供者",
        Some("备份传输方式"),
        colors,
        provider_pick.into(),
    )];

    let is_plugin = b.is_plugin_selected();
    let mut plugin_group: Option<Element<'a, Message>> = None;

    if is_plugin {
        // 插件配置表单（getSettingsSchema：text/secret/button）。
        if b.busy == Some("schema") {
            plugin_group = Some(settings_group(
                "插件配置",
                None::<String>,
                colors,
                vec![container(
                    text("正在加载插件配置…")
                        .size(13)
                        .color(colors.foreground_muted),
                )
                .width(Length::Fill)
                .into()],
            ));
        } else {
            let mut items: Vec<Element<'a, Message>> = Vec::new();
            for (field, value) in &b.plugin_fields {
                if field.ftype == "button" {
                    items.push(settings_item(
                        field.label.clone(),
                        field.help_text.clone(),
                        colors,
                        text_button(field.label.clone(), colors, Message::BackupTest).into(),
                    ));
                    continue;
                }
                let key = field.key.clone();
                let on_input = editable.then(|| {
                    let key = key.clone();
                    move |v: String| Message::BackupFieldChanged(key.clone(), v)
                });
                let input = text_input(field.placeholder.as_deref().unwrap_or(""), value)
                    .on_input_maybe(on_input)
                    .secure(field.ftype == "secret")
                    .style(move |_t, s| text_input_style(colors, s))
                    .width(Length::FillPortion(2));
                items.push(settings_item(
                    field.label.clone(),
                    field.help_text.clone(),
                    colors,
                    input.into(),
                ));
            }
            if items.is_empty() {
                items.push(
                    container(
                        text("该插件无可配置项")
                            .size(13)
                            .color(colors.foreground_muted),
                    )
                    .width(Length::Fill)
                    .into(),
                );
            }
            plugin_group = Some(settings_group("插件配置", None::<String>, colors, items));
        }
    } else {
        // 内置 WebDAV 连接配置。
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
        service_items.push(settings_item(
            "服务器地址",
            Some("WebDAV 根地址（含路径）"),
            colors,
            url_input.into(),
        ));
        service_items.push(settings_item(
            "用户名",
            None::<String>,
            colors,
            user_input.into(),
        ));
        service_items.push(settings_item(
            "密码",
            Some("明文保存于本地配置文件 (0600)"),
            colors,
            pass_input.into(),
        ));
        service_items.push(settings_item(
            "远端目录",
            Some("备份包存放的目录名"),
            colors,
            dir_input.into(),
        ));
        service_items.push(settings_item("连接", None::<String>, colors, test_control));
    }
    let mut groups = vec![settings_group(
        "备份服务",
        Some("备份内容为 rime 用户目录；backup 插件与 Android 通用"),
        colors,
        service_items,
    )];
    if let Some(g) = plugin_group {
        groups.push(g);
    }

    // 备份模式。
    let current_mode = MODES.get(b.mode as usize).copied().unwrap_or(MODES[0]);
    let mode_pick = pick_list(MODES, Some(current_mode), |chosen| {
        Message::BackupModeChanged(MODES.iter().position(|m| *m == chosen).unwrap_or(0) as u8)
    });
    groups.push(settings_group(
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
    ));

    // 备份操作。
    let op_control: Element<'a, Message> = match b.busy {
        Some("backup") => label("正在备份…", colors),
        Some("list") => label("正在获取远端列表…", colors),
        Some("restore") => label("正在恢复…", colors),
        Some("delete") => label("正在删除…", colors),
        Some("schema") => label("正在加载插件…", colors),
        Some(_) => label("正在连接…", colors),
        None => row![
            button_primary("立即备份", colors, Message::BackupNow),
            button_secondary("查看远端备份", colors, Message::BackupList),
        ]
        .spacing(8)
        .into(),
    };
    groups.push(settings_group(
        "备份操作",
        Some("恢复覆盖 rime 目录同名文件，重启输入法后生效"),
        colors,
        vec![settings_item(
            "操作",
            b.message.as_deref(),
            colors,
            op_control,
        )],
    ));

    // 远端备份列表。
    let mut remote_items: Vec<Element<'a, Message>> = Vec::new();
    if b.remote.is_empty() {
        remote_items.push(
            container(text("远端暂无备份").size(13).color(colors.foreground_muted))
                .width(Length::Fill)
                .into(),
        );
    } else {
        for entry in &b.remote {
            let mut sub = String::new();
            if let Some(size) = entry.size.filter(|s| *s >= 0) {
                sub.push_str(&format_size(size as u64));
            }
            if let Some(tag) = crate::backup::mode_tag_of(&entry.name) {
                if !sub.is_empty() {
                    sub.push_str(" · ");
                }
                sub.push_str(if tag == "full" { "全量" } else { "仅配置" });
            }
            let left = column![
                text(&entry.name).size(13).color(colors.foreground),
                text(sub).size(11).color(colors.foreground_muted),
            ]
            .spacing(2)
            .width(Length::Fill);
            let actions = row![
                text_button("恢复", colors, Message::BackupRestore(entry.id.clone())),
                text_button("删除", colors, Message::BackupDelete(entry.id.clone())),
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
