use crate::components::settings::{settings_group, settings_page};
use crate::components::widgets::semibold;
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{column, svg, text, Space};
use iced::{Alignment, Element, Length};

pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let meta = xime_config::app_metadata();
    settings_page(
        format!("关于 {}", meta.display_name),
        colors,
        vec![settings_group(
            format!("{} 输入法", meta.display_name),
            None::<String>,
            colors,
            vec![about_content(colors)],
        )],
    )
}

fn about_content(colors: &ThemeColors) -> Element<'static, Message> {
    let meta = xime_config::app_metadata();
    let logo: Element<'static, Message> = match crate::Assets::get("icons/xime.svg") {
        Some(f) => svg(svg::Handle::from_memory(f.data))
            .width(64)
            .height(64)
            .into(),
        None => Space::new().width(64).into(),
    };

    column![
        logo,
        text(meta.display_name)
            .size(16)
            .font(semibold())
            .color(colors.foreground),
        text(format!("版本 {}", meta.version))
            .size(12)
            .color(colors.foreground_muted),
        text("基于 Rime 引擎的五笔输入法")
            .size(12)
            .color(colors.foreground_muted),
        text("使用 librime + Iced 构建")
            .size(12)
            .color(colors.foreground_muted),
    ]
    .spacing(8)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .padding(16)
    .into()
}
