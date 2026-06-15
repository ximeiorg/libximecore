use crate::components::{SettingsControl, SettingsGroup, SettingsItem, SettingsPage};
use crate::state::SettingsState;
use crate::theme::ThemeColors;
use gpui::*;

pub fn render(
    _settings: &Entity<SettingsState>,
    colors: &ThemeColors,
) -> impl IntoElement {
    SettingsPage::new("同步", colors.clone())
        .group(
            SettingsGroup::new("WebDAV 同步", colors.clone())
                .description("通过 WebDAV 同步 Rime 配置到多台设备")
                .items(vec![
                    SettingsItem::new("服务器地址", SettingsControl::label("WebDAV URL"))
                        .description("https://example.com/remote.php/dav/"),
                    SettingsItem::new("用户名", SettingsControl::label("WebDAV 用户名"))
                        .description("输入您的 WebDAV 账户"),
                    SettingsItem::new("密码", SettingsControl::label("********"))
                        .description("输入您的 WebDAV 密码"),
                    SettingsItem::new("远程目录", SettingsControl::label("xime"))
                        .description("远程存储目录名称"),
                ]),
        )
}
