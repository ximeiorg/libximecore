#![cfg(feature = "clipboard-page")]
use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::switch;
use crate::components::widgets::{
    badge, button_primary, button_secondary, label, text_button, text_input_style,
};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{container, pick_list, row, text, text_input};
use iced::{Color, Element, Length};

/// 剪贴板同步配置：本地同步服务器（xime-sync-server）启停 + 认证。
pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let c = &settings.clipboard;

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

    let sp = &settings.sync_plugin;
    let mut sync_items: Vec<Element<'a, Message>> = Vec::new();

    // 启用开关。
    sync_items.push(settings_item(
        "启用剪贴板同步",
        Some("通过同步插件与远端设备双向同步（与 Android 插件通用）"),
        colors,
        switch(sp.enabled, colors, Message::SyncPluginEnabled),
    ));

    // 插件选择。
    if sp.plugins.is_empty() {
        sync_items.push(settings_item(
            "同步插件",
            Some("未安装 clipboard_sync 插件；请将插件目录放入 plugins/ 后重启设置程序"),
            colors,
            label("", colors),
        ));
    } else {
        let names: Vec<&str> = sp.plugins.iter().map(|p| p.name.as_str()).collect();
        let current = names.get(sp.plugin_index).copied().unwrap_or("");
        sync_items.push(settings_item(
            "同步插件",
            None::<String>,
            colors,
            pick_list(names, Some(current), |chosen| {
                Message::SyncPluginSelected(
                    sp.plugins
                        .iter()
                        .position(|p| p.name == chosen)
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

    let mut groups = vec![
        plugin_group,
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
        groups.push(settings_group(
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

    settings_page("剪贴板同步", colors, groups)
}
