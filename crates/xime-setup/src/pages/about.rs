use crate::components::settings::settings_page;
use crate::components::widgets::{
    button_secondary, card_shadow, card_style, medium, semibold, shorten_middle,
};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{column, container, row, svg, text, Space};
use iced::{border, Alignment, Background, Border, Color, Element, Length};

pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    let meta = xime_config::app_metadata();

    settings_page(
        format!("关于 {}", meta.display_name),
        &colors,
        vec![
            hero_card(colors),
            info_card(colors),
            license_card(colors),
        ],
    )
}

/// 头部卡片：图标 + 名称 + 版本 + 一句话简介 + 主要操作。
///
/// 之前这一页是居中竖排的文字柱（图标、名称、版本、两行描述逐行往下堆），
/// 既没有可做的操作，也没有信息层级——看着像安装器的收尾页。
/// 改成 HeroUI 的卡片头部结构：左边品牌块，右边横向排操作。
fn hero_card<'a>(colors: ThemeColors) -> Element<'a, Message> {
    let meta = xime_config::app_metadata();

    let logo: Element<'a, Message> = match crate::brand_assets() {
        Some(b) => svg(svg::Handle::from_memory(b.logo_svg))
            .width(52)
            .height(52)
            .into(),
        None => Space::new().width(52).height(52).into(),
    };

    // 图标底：浅主色方块，把 logo 从白卡片上托起来（HeroUI avatar 的处理方式）。
    let logo_tile = container(logo).padding(14).style(move |_| container::Style {
        background: Some(Background::Color(colors.primary_dim)),
        border: Border {
            color: Color { a: 0.16, ..colors.primary },
            width: 1.0,
            radius: border::radius(20.0),
        },
        ..container::Style::default()
    });

    let title = row![
        text(meta.display_name)
            .size(22)
            .font(semibold())
            .color(colors.foreground),
        // 文字用 primary_hover：浅色下比 primary 深一档、深色下比 primary 亮一档，
        // 两边都能从 tint 底上读出来（直接用 primary 在浅色里会发灰）。
        pill(
            format!("v{}", meta.version),
            colors.primary_hover,
            colors.primary_dim,
            colors,
        ),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let intro = column![
        title,
        text(meta.tagline)
            .size(14)
            .font(medium())
            .color(colors.foreground_muted),
    ]
    .spacing(6);

    let actions = row![
        button_secondary("打开数据目录", &colors, Message::OpenUserDataDir),
        button_secondary("项目主页", &colors, Message::OpenProjectHomepage),
    ]
    .spacing(8);

    container(
        row![logo_tile, intro, Space::new().width(Length::Fill), actions]
            .spacing(18)
            .align_y(Alignment::Center)
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .padding(20)
    .style(move |_| container::Style {
        shadow: card_shadow(&colors),
        ..card_style(&colors)
    })
    .into()
}

/// 应用信息：版本、引擎、平台、目录这些“想知道就查”的事实。
///
/// 这里没有复用 `settings_item`：那个组件左侧是「标签 + 描述」两行，
/// 右侧控件在整行里垂直居中，于是值会比标签低半个描述行——一排看过去
/// 值全在往下掉。这里让标签和值同处一行（同一基线）。
fn info_card<'a>(colors: ThemeColors) -> Element<'a, Message> {
    let meta = xime_config::app_metadata();
    let (_, user_dir) = xime_config::get_data_dirs();

    let mut body = column![
        text("应用信息")
            .size(16)
            .font(semibold())
            .color(colors.foreground),
    ]
    .spacing(4)
    .width(Length::Fill);

    let facts: Vec<(&str, String)> = vec![
        ("版本", meta.version.to_string()),
        ("输入方案引擎", "librime".to_string()),
        ("界面框架", "Iced · Rust".to_string()),
        ("运行平台", platform_label()),
        (
            "配置目录",
            shorten_middle(&user_dir.display().to_string(), 42),
        ),
        ("应用标识", meta.app_name.to_string()),
    ];

    for (name, value) in facts {
        body = body.push(fact_row(name, value, colors));
    }

    container(body)
        .width(Length::Fill)
        .padding([16, 18])
        .style(move |_| card_style(&colors))
        .into()
}

/// 一行事实：标签左、值右，同一基线。
fn fact_row<'a>(name: &'a str, value: String, colors: ThemeColors) -> Element<'a, Message> {
    container(
        row![
            text(name).size(14).font(medium()).color(colors.foreground),
            Space::new().width(Length::Fill),
            text(value).size(13).color(colors.foreground_muted),
        ]
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .padding([8, 0])
    .into()
}

/// 开源许可与技术栈。
fn license_card<'a>(colors: ThemeColors) -> Element<'a, Message> {
    let stack = row![
        pill(
            "librime",
            colors.foreground_muted,
            Color::TRANSPARENT,
            colors,
        ),
        pill("Iced", colors.foreground_muted, Color::TRANSPARENT, colors),
        pill("Rust", colors.foreground_muted, Color::TRANSPARENT, colors),
    ]
    .spacing(8);

    let body = column![
        text("开源许可")
            .size(16)
            .font(semibold())
            .color(colors.foreground),
        text("以 GNU 通用公共许可证第 3 版（GPL-3.0）发布，源码托管在 GitHub。")
            .size(13)
            .color(colors.foreground_muted),
        Space::new().height(2),
        stack,
    ]
    .spacing(8)
    .width(Length::Fill);

    container(body)
        .width(Length::Fill)
        .padding(18)
        .style(move |_| card_style(&colors))
        .into()
}

/// 圆形胶囊标签（版本号 / 技术栈）。
fn pill<'a>(
    label: impl Into<String>,
    fg: Color,
    bg: Color,
    colors: ThemeColors,
) -> Element<'a, Message> {
    container(text(label.into()).size(12).font(semibold()).color(fg))
        .padding([4, 10])
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                color: colors.border,
                width: 1.0,
                radius: border::radius(999.0),
            },
            ..container::Style::default()
        })
        .into()
}

/// 运行平台短标识，如 "macOS · arm64"。
fn platform_label() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    format!("{} · {}", os, std::env::consts::ARCH)
}
