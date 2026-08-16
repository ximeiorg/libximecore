use crate::components::widgets::{card_style, semibold};
use crate::theme::ThemeColors;
use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length};

/// 设置页：标题 + 一组卡片（分组）。
pub fn settings_page<'a, Message: 'a>(
    title: impl Into<String>,
    colors: &ThemeColors,
    groups: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let colors = *colors;
    let title = title.into();

    column![
        text(title)
            .size(20)
            .font(semibold())
            .color(colors.foreground),
        column(groups)
            .spacing(12)
            .width(Length::Fill)
            .height(Length::Shrink),
    ]
    .spacing(16)
    .padding(20)
    .width(Length::Fill)
    .into()
}

/// 设置分组卡片：标题 + 可选描述 + 条目/自定义内容。
pub fn settings_group<'a, Message: 'a>(
    title: impl Into<String>,
    description: Option<impl Into<String>>,
    colors: &ThemeColors,
    items: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let colors = *colors;
    let title = title.into();

    let mut body = column![text(title)
        .size(16)
        .font(semibold())
        .color(colors.foreground),]
    .spacing(8)
    .width(Length::Fill);

    if let Some(desc) = description {
        body = body.push(text(desc.into()).size(12).color(colors.foreground_muted));
    }

    for item in items {
        body = body.push(item);
    }

    container(body)
        .width(Length::Fill)
        .padding(16)
        .style(move |_| card_style(&colors))
        .into()
}

/// 设置条目：左侧 label + 描述，右侧控件。
pub fn settings_item<'a, Message: 'a>(
    label: impl Into<String>,
    description: Option<impl Into<String>>,
    colors: &ThemeColors,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    let colors = *colors;
    let label = label.into();

    let mut left = column![text(label).size(14).color(colors.foreground),]
        .spacing(4)
        .width(Length::Fill);

    if let Some(desc) = description {
        left = left.push(text(desc.into()).size(12).color(colors.foreground_muted));
    }

    row![left, control]
        .spacing(12)
        .align_y(Alignment::Center)
        .padding([12, 4])
        .width(Length::Fill)
        .into()
}
