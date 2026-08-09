use crate::components::settings::{settings_group, settings_page};
use crate::components::widgets::semibold;
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{column, svg, text, Space};
use iced::{Alignment, Element, Length};

pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    settings_page(
        "关于 Xime",
        colors,
        vec![settings_group(
            "Xime 输入法",
            None,
            colors,
            vec![about_content(colors)],
        )],
    )
}

fn about_content(colors: &ThemeColors) -> Element<'static, Message> {
    let logo: Element<'static, Message> = match crate::Assets::get("icons/xime.svg") {
        Some(f) => svg(svg::Handle::from_memory(f.data))
            .width(64)
            .height(64)
            .into(),
        None => Space::new().width(64).into(),
    };

    column![
        logo,
        text("Xime").size(16).font(semibold()).color(colors.foreground),
        text("版本 0.2.0").size(12).color(colors.foreground_muted),
        text("基于 Rime 引擎的五笔输入法").size(12).color(colors.foreground_muted),
        text("使用 librime + Iced 构建").size(12).color(colors.foreground_muted),
    ]
    .spacing(8)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .padding(16)
    .into()
}
