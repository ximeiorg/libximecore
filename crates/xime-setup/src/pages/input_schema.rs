use crate::components::widgets::{badge, button_danger, button_primary, card_style, semibold};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{button, column, container, row, text};
use iced::{border, Alignment, Background, Border, Color, Element, Length};

pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let tab = settings.input_schema.current_tab;
    let installed_ids = settings.market_schema.installed_ids.clone();

    let header = row![
        text("输入方案")
            .size(20)
            .font(semibold())
            .color(colors.foreground),
        iced::widget::Space::new().width(Length::Fill),
        button_primary("部署方案", colors, Message::DeploySchemas),
        button_primary("打开数据目录", colors, Message::OpenUserDataDir),
    ]
    .align_y(Alignment::Center)
    .spacing(12);

    let mut content = column![header, tab_bar(tab, colors)]
        .spacing(16)
        .padding(20)
        .width(Length::Fill);

    content = content.push(if tab == 0 {
        installed_tab(settings, colors)
    } else {
        downloads_tab(&installed_ids, colors)
    });

    content.into()
}

/// 分段标签：已安装 / 已下载。
fn tab_bar<'a>(active: usize, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    let labels = ["已安装", "已下载"];
    let mut bar = row![].spacing(2).padding(2);

    for (i, label) in labels.iter().enumerate() {
        let is_active = i == active;
        let label = *label;
        bar = bar.push(
            button(text(label).size(13).color(if is_active {
                colors.primary
            } else {
                colors.foreground_muted
            }))
            .padding([7, 16])
            .style(move |_theme, status| {
                let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: if is_active || hovered {
                        Some(Background::Color(colors.surface))
                    } else {
                        None
                    },
                    text_color: if is_active {
                        colors.primary
                    } else {
                        colors.foreground_muted
                    },
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: border::radius(8.0),
                    },
                    ..button::Style::default()
                }
            })
            .on_press(Message::SchemaTab(i)),
        );
    }

    container(bar)
        .width(Length::Shrink)
        .style(move |_| container::Style {
            background: Some(Background::Color(colors.surface_variant)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: border::radius(10.0),
            },
            ..container::Style::default()
        })
        .into()
}

/// 已安装方案列表 + 部署。
fn installed_tab<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    let schemas = settings.input_schema.available_schemas.clone();
    let selected = settings.input_schema.selected_schema;

    if schemas.is_empty() {
        return container(
            text("暂无已安装的方案")
                .size(13)
                .color(colors.foreground_muted),
        )
        .width(Length::Fill)
        .padding(24)
        .center_x(Length::Fill)
        .into();
    }

    let mut list = column![].spacing(6).width(Length::Fill);
    for (i, schema) in schemas.iter().enumerate() {
        let is_current = i == selected;
        let display = if schema.name.is_empty() {
            schema.schema_id.clone()
        } else {
            format!("{} ({})", schema.name, schema.schema_id)
        };

        let mut left = row![text(display).size(14).color(if is_current {
            colors.on_primary
        } else {
            colors.foreground
        }),]
        .spacing(8)
        .align_y(Alignment::Center);
        if is_current {
            left = left.push(badge(
                "当前",
                colors.on_primary,
                Color::from_rgba(1.0, 1.0, 1.0, 0.2),
            ));
        }
        left = left.push(text("设置").size(12).color(if is_current {
            colors.primary
        } else {
            colors.foreground_muted
        }));

        let row_item = button(left)
            .on_press(Message::SelectSchema(i))
            .width(Length::Fill)
            .height(Length::Shrink)
            .padding([10, 12])
            .style(move |_theme, status| {
                let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: Some(Background::Color(if is_current {
                        colors.primary
                    } else if hovered {
                        colors.surface_hover
                    } else {
                        colors.surface
                    })),
                    text_color: if is_current {
                        colors.on_primary
                    } else {
                        colors.foreground
                    },
                    border: Border {
                        color: if is_current {
                            Color::TRANSPARENT
                        } else {
                            colors.border
                        },
                        width: 1.0,
                        radius: border::radius(8.0),
                    },
                    ..button::Style::default()
                }
            });

        list = list.push(row_item);
    }

    if let Some(msg) = &settings.market_schema.install_message {
        list = list.push(text(msg.clone()).size(12).color(colors.error));
    }

    list.into()
}

/// 已下载方案包列表。
fn downloads_tab<'a>(installed_ids: &[String], colors: &'a ThemeColors) -> Element<'a, Message> {
    let pkg_ids = scan_market_dir();

    if pkg_ids.is_empty() {
        return column![
            text("暂无已下载的方案包")
                .size(14)
                .color(colors.foreground_muted),
            text("请前往「扩展商店」下载")
                .size(13)
                .color(colors.foreground_muted),
        ]
        .spacing(8)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .padding(48)
        .into();
    }

    let mut list = column![].spacing(8).width(Length::Fill);
    for pkg_id in &pkg_ids {
        let installed = installed_ids.contains(pkg_id);
        list = list.push(package_card(pkg_id, installed, colors));
    }
    list.into()
}

fn package_card<'a>(
    pkg_id: &str,
    installed: bool,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let colors = *colors;
    let sid = pkg_id.to_string();
    let action: Element<'a, Message> = if installed {
        button_danger("卸载", &colors, Message::UninstallSchema(sid)).into()
    } else {
        button_primary("安装", &colors, Message::InstallSchema(sid)).into()
    };

    container(
        row![
            column![
                text(pkg_id.to_string()).size(14).color(colors.foreground),
                text(if installed {
                    "已安装至输入法"
                } else {
                    "已下载，未安装"
                })
                .size(12)
                .color(colors.foreground_muted),
            ]
            .spacing(2)
            .width(Length::Fill),
            action,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .padding(12),
    )
    .width(Length::Fill)
    .style(move |_| card_style(&colors))
    .into()
}

/// 扫描本地市场缓存目录中的已下载方案包。
fn scan_market_dir() -> Vec<String> {
    let market_dir = if cfg!(debug_assertions) {
        let mut p = std::env::current_exe().unwrap_or_default();
        p.pop();
        while !p.join("Cargo.toml").exists() && p.parent().is_some() {
            p.pop();
        }
        p.join("target").join("debug").join("market")
    } else {
        std::env::current_exe()
            .unwrap_or_default()
            .parent()
            .map(|d| d.join("market"))
            .unwrap_or_else(|| std::path::PathBuf::from("market"))
    };

    let mut pkg_ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&market_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.') {
                continue;
            }
            if entry.path().is_dir() {
                let has_archive = std::fs::read_dir(entry.path())
                    .map(|mut e| {
                        e.any(|e| {
                            e.ok()
                                .and_then(|e| {
                                    e.file_name()
                                        .to_str()
                                        .map(|n| n.ends_with(".zip") || n.ends_with(".tar.gz"))
                                })
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
                if has_archive {
                    pkg_ids.push(name_str.to_string());
                }
            }
        }
    }
    pkg_ids
}
