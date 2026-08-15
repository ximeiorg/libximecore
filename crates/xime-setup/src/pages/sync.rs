#![cfg(target_os = "linux")]
use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::label;
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::Element;

pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    settings_page(
        "同步",
        colors,
        vec![settings_group(
            "WebDAV 同步",
            Some("通过 WebDAV 同步 Rime 配置到多台设备"),
            colors,
            vec![
                settings_item(
                    "服务器地址",
                    Some("https://example.com/remote.php/dav/"),
                    colors,
                    label("WebDAV URL", colors),
                ),
                settings_item(
                    "用户名",
                    Some("输入您的 WebDAV 账户"),
                    colors,
                    label("WebDAV 用户名", colors),
                ),
                settings_item(
                    "密码",
                    Some("输入您的 WebDAV 密码"),
                    colors,
                    label("********", colors),
                ),
                settings_item(
                    "远程目录",
                    Some("远程存储目录名称"),
                    colors,
                    label(xime_config::app_metadata().config_dir_name, colors),
                ),
            ],
        )],
    )
}
