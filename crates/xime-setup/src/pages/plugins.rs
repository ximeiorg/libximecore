use crate::components::widgets::{
    button_danger, card_style, semibold, switch, text_button, RADIUS_MD,
};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{column, container, row, text};
use iced::{border, Alignment, Background, Border, Color, Element, Length};

/// 插件管理页：已安装插件列表 + 启用/禁用/卸载。
pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let installed = &settings.market_plugin.installed;

    let mut content = column![row![
        text("插件管理")
            .size(20)
            .font(semibold())
            .color(colors.foreground)
            .width(Length::Fill),
        text_button("刷新", colors, Message::RefreshPlugins),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill),]
    .spacing(16)
    .padding(20)
    .width(Length::Fill);

    if installed.is_empty() {
        content = content.push(empty_state(colors));
    } else {
        let mut list = column![].spacing(10).width(Length::Fill);
        for plugin in installed {
            list = list.push(plugin_row(plugin, settings, colors));
        }
        content = content.push(list);
    }

    content.into()
}

fn empty_state<'a>(colors: &'a ThemeColors) -> Element<'a, Message> {
    container(
        column![
            text("暂无已安装的插件")
                .size(14)
                .color(colors.foreground_muted),
            text("从扩展商店下载插件后会显示在这里")
                .size(12)
                .color(colors.foreground_faint),
        ]
        .spacing(8)
        .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(32)
    .center_x(Length::Fill)
    .into()
}

fn plugin_row<'a>(
    plugin: &'a xime_plugin::PluginRecord,
    settings: &'a SettingsState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let enabled = plugin.enabled;
    let type_label = plugin_type_label(&plugin.plugin_type);
    let glyph = type_label.chars().next().unwrap_or('插');

    let confirming = settings.plugin_uninstall_confirm.as_deref() == Some(plugin.id.as_str());

    let mut right = row![switch(enabled, colors, move |on| {
        Message::TogglePlugin(plugin.id.clone(), on)
    }),]
    .spacing(8)
    .align_y(Alignment::Center);

    if confirming {
        right = right.push(
            row![
                text_button("取消", colors, Message::CancelUninstallPlugin),
                button_danger(
                    "确认卸载",
                    colors,
                    Message::UninstallPlugin(plugin.id.clone())
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    } else {
        right = right.push(button_danger(
            "卸载",
            colors,
            Message::ConfirmUninstallPlugin(plugin.id.clone()),
        ));
    }

    let body = row![
        glyph_box(glyph, plugin_type_color(&plugin.plugin_type, colors)),
        column![
            text(if plugin.name.is_empty() {
                plugin.id.clone()
            } else {
                plugin.name.clone()
            })
            .size(15)
            .font(semibold())
            .color(colors.foreground),
            text(if type_label.is_empty() {
                format!("v{}", plugin.version)
            } else {
                format!("{} · v{}", type_label, plugin.version)
            })
            .size(12)
            .color(colors.foreground_muted),
        ]
        .spacing(2)
        .width(Length::Fill),
        right,
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(body)
        .width(Length::Fill)
        .padding(14)
        .style(move |_| card_style(colors))
        .into()
}

/// 插件类型展示名（与扩展商店分类一致）。
fn plugin_type_label(plugin_type: &str) -> String {
    match plugin_type {
        "speech" => "语音".to_string(),
        "emoji" => "表情".to_string(),
        "prediction" => "联想".to_string(),
        "clipboard" => "剪贴板".to_string(),
        other if !other.is_empty() => other.to_string(),
        _ => String::new(),
    }
}

fn plugin_type_color(plugin_type: &str, colors: &ThemeColors) -> (Color, Color) {
    match plugin_type {
        "emoji" => (colors.tertiary_dim, colors.tertiary),
        "speech" => (colors.primary_dim, colors.primary),
        "prediction" => (colors.secondary_dim, colors.secondary),
        _ => (colors.surface_variant, colors.foreground_muted),
    }
}

fn glyph_box<'a>(glyph: char, icon: (Color, Color)) -> Element<'a, Message> {
    let (container_bg, content_color) = icon;
    container(
        text(glyph.to_string())
            .size(13)
            .font(semibold())
            .color(content_color),
    )
    .width(28)
    .height(28)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |_| container::Style {
        background: Some(Background::Color(container_bg)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: border::radius(RADIUS_MD),
        },
        ..container::Style::default()
    })
    .into()
}
