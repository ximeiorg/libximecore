use crate::components::settings::{settings_group, settings_page};
use crate::components::widgets::{
    badge, button_primary, card_style, semibold, text_button,
};
use crate::state::{MarketSchema, MarketSchemaState, Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{
    button, column, container, row, text,
};
use iced::{border, Alignment, Background, Border, Color, Element, Length};

pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let market = &settings.market_schema;

    let content: Element<'a, Message> = if market.loading && !market.loaded {
        center_msg(colors, "正在加载方案列表…")
    } else if let Some(error) = &market.error {
        error_view(colors, error)
    } else if market.loaded {
        schema_list(market, colors)
    } else {
        center_msg(colors, "正在加载方案列表…")
    };

    let mut groups = vec![settings_group(
        "方案列表",
        Some("从方案市场下载并安装输入方案"),
        colors,
        vec![content],
    )];

    if let Some(msg) = &market.install_message {
        groups.push(settings_group(
            "提示",
            None,
            colors,
            vec![text(msg.clone()).size(12).color(colors.error).into()],
        ));
    }

    settings_page("方案市场", colors, groups)
}

fn center_msg<'a>(colors: &'a ThemeColors, msg: &'a str) -> Element<'a, Message> {
    container(text(msg).size(14).color(colors.foreground_muted))
        .width(Length::Fill)
        .padding(32)
        .center_x(Length::Fill)
        .into()
}

fn error_view<'a>(colors: &'a ThemeColors, error: &'a str) -> Element<'a, Message> {
    column![
        text(error.to_string()).size(14).color(colors.foreground_muted),
        text_button("重试", colors, Message::MarketRetry),
    ]
    .spacing(12)
    .align_x(Alignment::Start)
    .width(Length::Fill)
    .padding(24)
    .into()
}

fn schema_list<'a>(
    market: &'a MarketSchemaState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let schemas = market.schemas.clone();
    let installed = &market.installed_ids;
    let downloaded = &market.downloaded_ids;
    let downloading = market.downloading.as_deref();
    let installing = market.installing.as_deref();

    let mut list = column![].spacing(8).width(Length::Fill);
    for schema in &schemas {
        let is_installed = installed.contains(&schema.id);
        let is_downloaded = downloaded.contains(&schema.id);
        let is_downloading = downloading == Some(schema.id.as_str());
        let is_installing = installing == Some(schema.id.as_str());

        list = list.push(schema_card(
            schema,
            is_installed,
            is_downloaded,
            is_downloading,
            is_installing,
            colors,
        ));
    }
    list.into()
}

fn schema_card<'a>(
    schema: &MarketSchema,
    installed: bool,
    downloaded: bool,
    downloading: bool,
    installing: bool,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let version = schema.current_version.as_deref().unwrap_or("latest");

    let size_label = schema
        .versions
        .iter()
        .find(|v| {
            v.version == version
                || (schema.current_version.is_none() && v.version == "latest")
        })
        .or_else(|| schema.versions.first())
        .and_then(|v| {
            v.download_url
                .first()
                .and_then(|u| u.size.as_deref())
                .or(v.size.as_deref())
                .map(format_size)
        })
        .unwrap_or_default();

    let deps = schema.dependencies.clone().unwrap_or_default();

    let mut header = row![
        text(if schema.name.is_empty() {
            schema.id.clone()
        } else {
            schema.name.clone()
        })
        .size(16)
        .font(semibold())
        .color(colors.foreground),
        text(version.to_string()).size(12).color(colors.primary),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if schema.schema_type == "built-in" {
        header = header.push(badge("内置", colors.on_primary, colors.primary));
    }

    let mut body = column![header].spacing(8).width(Length::Fill);

    if !schema.description.is_empty() {
        body = body.push(
            text(schema.description.to_string())
                .size(13)
                .color(colors.foreground_muted),
        );
    }

    let author_line = if schema.author.is_empty() {
        schema.tags.join("、")
    } else {
        format!("作者：{}　{}", schema.author, schema.tags.join("、"))
    };
    body = body.push(text(author_line).size(12).color(colors.foreground_muted));

    if !deps.is_empty() {
        let mut deps_row = row![].spacing(4);
        for dep in deps.iter().take(4) {
            deps_row = deps_row.push(pill(dep, colors));
        }
        body = body.push(deps_row);
    }

    if let Some(warning) = &schema.warning {
        if !warning.is_empty() {
            body = body.push(
                container(text(warning.to_string()).size(12).color(colors.foreground))
                    .width(Length::Fill)
                    .padding(10)
                    .style(move |_| container::Style {
                        background: Some(Background::Color(colors.error_dim)),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: border::radius(6.0),
                        },
                        ..container::Style::default()
                    }),
            );
        }
    }

    let size_text = if !size_label.is_empty() {
        format!("大小: {}", size_label)
    } else {
        String::new()
    };

    body = body.push(
        row![
            container(text(size_text).size(12).color(colors.foreground_muted))
                .width(Length::Fill),
            action_button(
                schema.id.clone(),
                installed,
                downloaded,
                downloading,
                installing,
                colors,
            ),
        ]
        .width(Length::Fill)
        .align_y(Alignment::Center),
    );

    container(body)
        .width(Length::Fill)
        .padding(14)
        .style(move |_| card_style(colors))
        .into()
}

fn action_button<'a>(
    schema_id: String,
    installed: bool,
    downloaded: bool,
    downloading: bool,
    installing: bool,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let colors = *colors;
    if installing {
        button(text("安装中…").size(13).color(colors.foreground_muted))
            .padding([6, 14])
            .style(move |_theme, _status| {
                crate::components::widgets::secondary_button_style(
                    &colors,
                    button::Status::Disabled,
                )
            })
            .into()
    } else if downloading {
        button(text("下载中…").size(13).color(colors.foreground_muted))
            .padding([6, 14])
            .style(move |_theme, _status| {
                crate::components::widgets::secondary_button_style(
                    &colors,
                    button::Status::Disabled,
                )
            })
            .into()
    } else if installed {
        row![
            button_primary("部署", &colors, Message::DeploySchemas),
            text("已安装").size(12).color(colors.foreground_muted),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else if downloaded {
        button_primary("安装", &colors, Message::InstallSchema(schema_id)).into()
    } else {
        button_primary("下载", &colors, Message::DownloadSchema(schema_id)).into()
    }
}

fn pill<'a>(label: &str, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    container(text(label.to_string()).size(11).color(colors.foreground_muted))
        .padding([2, 8])
        .style(move |_| container::Style {
            background: Some(Background::Color(colors.surface_variant)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: border::radius(6.0),
            },
            ..container::Style::default()
        })
        .into()
}

fn format_size(s: &str) -> String {
    let s = s.trim().to_lowercase();
    if s.ends_with("kb") || s.ends_with("mb") || s.ends_with("gb") {
        s.to_string()
    } else if let Ok(bytes) = s.parse::<u64>() {
        if bytes > 1024 * 1024 * 1024 {
            format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        } else if bytes > 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else if bytes > 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{} B", bytes)
        }
    } else {
        s.to_string()
    }
}
