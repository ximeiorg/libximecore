pub mod about;
pub mod appearance;
pub mod dictionary;
pub mod hotkeys;
pub mod input_schema;
pub mod plugins;
pub mod store;

#[cfg(feature = "clipboard-page")]
pub mod clipboard;
#[cfg(feature = "pair-page")]
pub mod pair;
#[cfg(feature = "smart-suggestion-page")]
pub mod smart_suggestion;
#[cfg(target_os = "linux")]
pub mod sync;

use crate::components::widgets::{nav_button_style, scroll_style, semibold, sidebar_style};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{button, column, container, row, scrollable, svg, text, Space};
use iced::{Alignment, Element, Length};

pub fn sidebar_items() -> Vec<(&'static str, &'static str)> {
    let mut items = vec![
        ("icons/keyboard.svg", "输入方案"),
        ("icons/palette.svg", "外观"),
        ("icons/command.svg", "快捷键"),
        ("icons/word.svg", "词典"),
        ("icons/store.svg", "扩展商店"),
        ("icons/extension.svg", "插件管理"),
    ];

    #[cfg(feature = "smart-suggestion-page")]
    items.push(("icons/thinking.svg", "智能联想"));

    #[cfg(target_os = "linux")]
    items.push(("icons/sync.svg", "同步"));

    #[cfg(feature = "pair-page")]
    items.push(("icons/sync.svg", "设备关联"));

    #[cfg(feature = "clipboard-page")]
    items.push(("icons/clipboard.svg", "剪贴板"));

    items.push(("icons/about.svg", "关于"));
    items
}

/// 侧栏导航。
pub fn sidebar(current: usize, colors: &ThemeColors) -> Element<'static, Message> {
    let items = sidebar_items();
    let colors = *colors;

    let mut nav = column![
        brand(&colors),
        Space::new().height(20),
        text("菜单").size(11).color(colors.foreground_faint),
        Space::new().height(4),
    ]
    .spacing(2)
    .padding([20, 14]);

    for (i, (icon_path, name)) in items.iter().enumerate() {
        nav = nav.push(nav_button(icon_path, name, i, i == current, &colors));
    }

    container(nav)
        .width(200)
        .height(Length::Fill)
        .style(move |_| sidebar_style(&colors))
        .into()
}

/// 品牌区：应用图标 + 名称 + 副标题。
fn brand(colors: &ThemeColors) -> Element<'static, Message> {
    let logo: Element<'static, Message> = match crate::Assets::get("icons/xime.svg") {
        Some(f) => svg(svg::Handle::from_memory(f.data))
            .width(28)
            .height(28)
            .into(),
        None => Space::new().width(28).into(),
    };
    row![
        logo,
        column![
            text(xime_config::app_metadata().display_name)
                .size(15)
                .font(semibold())
                .color(colors.foreground),
            text("输入法设置").size(11).color(colors.foreground_muted),
        ]
        .spacing(1),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

/// 导航项：图标 + 文字。
fn nav_button(
    icon_path: &'static str,
    label: &'static str,
    index: usize,
    active: bool,
    colors: &ThemeColors,
) -> Element<'static, Message> {
    let colors = *colors;
    let icon: Element<'static, Message> = match crate::Assets::get(icon_path) {
        Some(f) => svg(svg::Handle::from_memory(f.data))
            .width(16)
            .height(16)
            .into(),
        None => Space::new().width(16).into(),
    };
    button(
        row![
            icon,
            text(label).size(13).color(if active {
                colors.primary
            } else {
                colors.foreground_muted
            }),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .on_press(Message::PageSelected(index))
    .width(Length::Fill)
    .padding([8, 10])
    .style(move |_theme, status| nav_button_style(&colors, status, active))
    .into()
}

/// 当前页面内容。
pub fn page_content<'a>(
    settings: &'a SettingsState,
    page: usize,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let items = sidebar_items();
    let page = page.min(items.len().saturating_sub(1));

    match items[page].1 {
        "输入方案" => input_schema::view(settings, colors),
        "外观" => appearance::view(settings, colors),
        "快捷键" => hotkeys::view(settings, colors),
        "词典" => dictionary::view(settings, colors),
        "扩展商店" => store::view(settings, colors),
        "插件管理" => plugins::view(settings, colors),
        #[cfg(feature = "smart-suggestion-page")]
        "智能联想" => smart_suggestion::view(settings, colors),
        #[cfg(target_os = "linux")]
        "同步" => sync::view(settings, colors),
        #[cfg(feature = "pair-page")]
        "设备关联" => pair::view(settings, colors),
        #[cfg(feature = "clipboard-page")]
        "剪贴板" => clipboard::view(settings, colors),
        _ => about::view(settings, colors),
    }
}

pub fn scrollable_content<'a>(
    content: impl Into<Element<'a, Message>>,
    colors: &ThemeColors,
) -> Element<'a, Message> {
    let colors = *colors;
    scrollable(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |theme, status| scroll_style(&colors, theme, status))
        .into()
}
