use crate::components::widgets::{
    badge, button_danger, button_primary, card_style, semibold, switch, text_button,
};
use crate::state::{
    MarketModel, MarketModelState, MarketPlugin, MarketPluginState, MarketSchema,
    MarketSchemaState, Message, SettingsState,
};
use crate::theme::ThemeColors;
use iced::widget::{button, column, container, pick_list, row, scrollable, text};
use iced::{border, Alignment, Background, Border, Color, Element, Length};

/// 模型分类 → 展示标签（对应索引中的 category 字段）。
const MODEL_CATEGORIES: [(&str, &str); 4] = [
    ("prediction", "联想"),
    ("handwriting", "手写"),
    ("asr", "语音"),
    ("other", "其他"),
];

/// 插件类型 → 展示标签（对应索引中的 pluginType 字段）。
const PLUGIN_CATEGORIES: [(&str, &str); 4] = [
    ("speech", "语音"),
    ("emoji", "表情"),
    ("prediction", "联想"),
    ("other", "其他"),
];

pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let store = &settings.market_schema;
    let tab = store.store_tab;

    let mut content = column![header(colors), tab_bar(tab, colors),]
        .spacing(16)
        .padding(20)
        .width(Length::Fill);

    content = content.push(if tab == 0 {
        schemes_tab(settings, colors)
    } else if tab == 1 {
        models_tab(settings, colors)
    } else {
        plugins_tab(settings, colors)
    });

    content.into()
}

/// 页头：标题 + 刷新按钮。
fn header<'a>(colors: &'a ThemeColors) -> Element<'a, Message> {
    row![
        text("扩展商店")
            .size(20)
            .font(semibold())
            .color(colors.foreground)
            .width(Length::Fill),
        text_button("刷新", colors, Message::MarketRetry),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

/// 一级 Tab：方案 / 模型 / 插件。
fn tab_bar<'a>(active: usize, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    let labels = ["方案", "模型", "插件"];
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
            .on_press(Message::StoreTab(i)),
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

/// 分类筛选 chips（横向滚动），第一个恒为「全部」。
fn chip_bar<'a>(
    mut tags: Vec<String>,
    selected: &'a Option<String>,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let mut options: Vec<String> = Vec::with_capacity(tags.len() + 1);
    options.push(String::new());
    options.append(&mut tags);

    let mut chips = row![].spacing(6);
    for tag in options {
        let is_active = selected.as_deref() == Some(tag.as_str());
        chips = chips.push(chip(tag, is_active, colors));
    }

    scrollable(chips)
        .horizontal()
        .width(Length::Fill)
        .height(Length::Shrink)
        .into()
}

fn chip<'a>(tag: String, active: bool, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    let label = if tag.is_empty() {
        "全部".to_string()
    } else {
        tag.clone()
    };
    button(text(label).size(12).color(if active {
        colors.on_primary
    } else {
        colors.foreground_muted
    }))
    .padding([5, 12])
    .style(move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(if active {
                colors.primary
            } else if hovered {
                colors.surface_hover
            } else {
                colors.surface_variant
            })),
            text_color: if active {
                colors.on_primary
            } else {
                colors.foreground_muted
            },
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: border::radius(999.0),
            },
            ..button::Style::default()
        }
    })
    .on_press(Message::StoreTagSelected(tag))
    .into()
}

// ---- 方案 Tab ----

fn schemes_tab<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let store = &settings.market_schema;

    let content: Element<'a, Message> = if store.loading && !store.loaded {
        center_msg(colors, "正在加载方案列表…")
    } else if let Some(error) = &store.error {
        error_view(colors, error)
    } else if store.loaded {
        schema_list(store, colors)
    } else {
        center_msg(colors, "正在加载方案列表…")
    };

    let mut column = column![content].spacing(10).width(Length::Fill);

    if let Some(msg) = &store.install_message {
        column = column.push(text(msg.clone()).size(12).color(colors.error));
    }
    if store.loaded {
        column = column.push(footer(
            store.schemas.len(),
            "方案",
            &store.updated_at,
            colors,
        ));
    }

    column.into()
}

fn schema_list<'a>(store: &'a MarketSchemaState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let mut tags: Vec<String> = Vec::new();
    for schema in &store.schemas {
        for tag in &schema.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
    }

    let schemas: Vec<&MarketSchema> = store
        .schemas
        .iter()
        .filter(|s| {
            store
                .selected_tag
                .as_ref()
                .map(|t| s.tags.contains(t))
                .unwrap_or(true)
        })
        .collect();

    if schemas.is_empty() {
        return center_msg(colors, "没有匹配的方案");
    }

    let mut list = column![].spacing(8).width(Length::Fill);
    if !tags.is_empty() {
        list = list.push(chip_bar(tags, &store.selected_tag, colors));
    }
    for schema in schemas {
        list = list.push(schema_card(schema, store, colors));
    }
    list.into()
}

fn schema_card<'a>(
    schema: &'a MarketSchema,
    store: &'a MarketSchemaState,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let installed = store.installed_ids.contains(&schema.id);
    let downloaded = store.downloaded_ids.contains(&schema.id);
    let downloading = store.downloading.as_deref() == Some(schema.id.as_str());
    let installing = store.installing.as_deref() == Some(schema.id.as_str());
    let progress = if downloading {
        store.download_progress
    } else {
        None
    };
    let selected_version = store.selected_versions.get(&schema.id).cloned();

    let versions: Vec<String> = schema.versions.iter().map(|v| v.version.clone()).collect();
    let version = selected_version
        .or_else(|| schema.current_version.clone())
        .unwrap_or_else(|| versions.first().cloned().unwrap_or_else(|| "latest".into()));

    let size_label = schema
        .versions
        .iter()
        .find(|v| v.version == version)
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
    let glyph = schema
        .tags
        .first()
        .and_then(|t| t.chars().next())
        .or_else(|| schema.name.chars().next())
        .unwrap_or('方');

    let mut title = row![
        glyph_box(glyph, colors),
        column![row![
            text(if schema.name.is_empty() {
                schema.id.clone()
            } else {
                schema.name.clone()
            })
            .size(15)
            .font(semibold())
            .color(colors.foreground),
            text(version.to_string()).size(12).color(colors.primary),
        ]
        .spacing(8)
        .align_y(Alignment::Center),]
        .width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    if schema.schema_type == "built-in" {
        title = title.push(badge("内置", colors.on_primary, colors.primary));
    }

    let mut body = column![title].spacing(8).width(Length::Fill);

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
    if !author_line.trim().is_empty() {
        body = body.push(text(author_line).size(12).color(colors.foreground_muted));
    }

    if !deps.is_empty() {
        let mut deps_row = row![].spacing(4);
        for dep in deps.iter().take(4) {
            deps_row = deps_row.push(pill(dep, colors));
        }
        body = body.push(deps_row);
    }

    if let Some(warning) = &schema.warning {
        if !warning.is_empty() {
            body = body.push(warning_box(warning, colors));
        }
    }

    let size_text = if !size_label.is_empty() {
        format!("大小: {}", size_label)
    } else {
        String::new()
    };

    body = body.push(
        row![
            column![
                if size_text.is_empty() {
                    text("").size(12)
                } else {
                    text(size_text).size(12).color(colors.foreground_muted)
                },
                version_selector(
                    schema.id.clone(),
                    versions,
                    version,
                    colors,
                    Message::SchemaVersionSelected
                ),
            ]
            .spacing(4),
            container(text("")).width(Length::Fill),
            schema_action(
                schema.id.clone(),
                installed,
                downloaded,
                downloading,
                installing,
                progress,
                colors,
            ),
        ]
        .width(Length::Fill)
        .align_y(Alignment::End),
    );

    container(body)
        .width(Length::Fill)
        .padding(14)
        .style(move |_| card_style(colors))
        .into()
}

fn schema_action<'a>(
    schema_id: String,
    installed: bool,
    downloaded: bool,
    downloading: bool,
    installing: bool,
    progress: Option<f32>,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let colors = *colors;
    if installing {
        disabled_button("安装中…", colors)
    } else if downloading {
        let label = match progress {
            Some(p) => format!("下载中 {:.0}%", p * 100.0),
            None => "下载中…".to_string(),
        };
        disabled_button(label, colors)
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

// ---- 模型 Tab ----

fn models_tab<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let store = &settings.market_model;

    let content: Element<'a, Message> = if store.loading && !store.loaded {
        center_msg(colors, "正在加载模型列表…")
    } else if let Some(error) = &store.error {
        error_view(colors, error)
    } else if store.loaded {
        model_list(store, colors)
    } else {
        center_msg(colors, "正在加载模型列表…")
    };

    let mut column = column![content].spacing(10).width(Length::Fill);

    if let Some(msg) = &store.install_message {
        column = column.push(text(msg.clone()).size(12).color(colors.error));
    }
    if store.loaded {
        column = column.push(footer(
            store.models.len(),
            "模型",
            &store.updated_at,
            colors,
        ));
    }

    column.into()
}

fn model_list<'a>(store: &'a MarketModelState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let tags: Vec<String> = MODEL_CATEGORIES
        .iter()
        .filter(|(key, _)| store.models.iter().any(|m| m.category == *key))
        .map(|(_, label)| label.to_string())
        .collect();

    let models: Vec<&MarketModel> = store
        .models
        .iter()
        .filter(|m| {
            store
                .selected_tag
                .as_ref()
                .map(|t| model_category_label(&m.category) == *t)
                .unwrap_or(true)
        })
        .collect();

    if models.is_empty() {
        return center_msg(colors, "没有匹配的模型");
    }

    let mut list = column![].spacing(8).width(Length::Fill);
    if !tags.is_empty() {
        list = list.push(chip_bar(tags, &store.selected_tag, colors));
    }
    for model in models {
        let is_downloaded = store.downloaded_ids.contains(&model.id);
        let is_downloading = store.downloading.as_deref() == Some(model.id.as_str());
        let progress = if is_downloading {
            store.download_progress
        } else {
            None
        };
        list = list.push(model_card(
            model,
            is_downloaded,
            is_downloading,
            progress,
            store.selected_versions.get(&model.id).cloned(),
            colors,
        ));
    }
    list.into()
}

fn model_card<'a>(
    model: &'a MarketModel,
    downloaded: bool,
    downloading: bool,
    progress: Option<f32>,
    selected_version: Option<String>,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let versions: Vec<String> = model.versions.iter().map(|v| v.version.clone()).collect();
    let version = selected_version
        .or_else(|| model.current_version.clone())
        .unwrap_or_else(|| versions.first().cloned().unwrap_or_default());

    let size_label = model
        .versions
        .iter()
        .find(|v| v.version == version)
        .or_else(|| model.versions.first())
        .map(|v| v.size.as_deref().map(format_size).unwrap_or_default())
        .or_else(|| {
            if model.size.is_empty() {
                None
            } else {
                Some(format_size(&model.size))
            }
        })
        .unwrap_or_default();

    let category_label = model_category_label(&model.category);
    let glyph = category_label
        .chars()
        .next()
        .or_else(|| model.name.chars().next())
        .unwrap_or('模');

    let mut meta = String::new();
    if !category_label.is_empty() {
        meta.push_str(&category_label);
    }
    if !size_label.is_empty() {
        if !meta.is_empty() {
            meta.push_str(" · ");
        }
        meta.push_str(size_label.as_str());
    }

    let mut body = column![row![
        glyph_box(glyph, colors),
        column![
            text(if model.name.is_empty() {
                model.id.clone()
            } else {
                model.name.clone()
            })
            .size(15)
            .font(semibold())
            .color(colors.foreground),
            if model.description.is_empty() {
                text("").size(12)
            } else {
                text(model.description.to_string())
                    .size(13)
                    .color(colors.foreground_muted)
            },
            if meta.is_empty() {
                text("").size(12)
            } else {
                text(meta).size(12).color(colors.foreground_muted)
            },
        ]
        .spacing(3)
        .width(Length::Fill),
        model_action(model.id.clone(), downloaded, downloading, progress, colors,),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .width(Length::Fill),]
    .spacing(8)
    .width(Length::Fill);

    if !model.versions.is_empty() {
        body = body.push(
            row![
                version_selector(
                    model.id.clone(),
                    versions,
                    version,
                    colors,
                    Message::ModelVersionSelected
                ),
                container(text("")).width(Length::Fill),
            ]
            .align_y(Alignment::Center),
        );
    }

    container(body)
        .width(Length::Fill)
        .padding(14)
        .style(move |_| card_style(colors))
        .into()
}

fn model_action<'a>(
    model_id: String,
    downloaded: bool,
    downloading: bool,
    progress: Option<f32>,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let colors = *colors;
    if downloading {
        let label = match progress {
            Some(p) => format!("下载中 {:.0}%", p * 100.0),
            None => "下载中…".to_string(),
        };
        disabled_button(label, colors)
    } else if downloaded {
        row![
            text("已安装").size(12).color(colors.foreground_muted),
            button_danger("删除", &colors, Message::DeleteModel(model_id)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else {
        button_primary("获取", &colors, Message::DownloadModel(model_id)).into()
    }
}

fn model_category_label(category: &str) -> String {
    MODEL_CATEGORIES
        .iter()
        .find(|(key, _)| *key == category)
        .map(|(_, label)| label.to_string())
        .unwrap_or_default()
}

// ---- 插件 Tab ----

fn plugins_tab<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let store = &settings.market_plugin;

    let content: Element<'a, Message> = if store.loading && !store.loaded {
        center_msg(colors, "正在加载插件列表…")
    } else if let Some(error) = &store.error {
        error_view(colors, error)
    } else if store.loaded {
        plugin_list(store, colors)
    } else {
        center_msg(colors, "正在加载插件列表…")
    };

    let mut column = column![content].spacing(10).width(Length::Fill);

    if let Some(msg) = &store.install_message {
        column = column.push(text(msg.clone()).size(12).color(colors.error));
    }
    if store.loaded {
        column = column.push(footer(
            store.plugins.len(),
            "插件",
            &store.updated_at,
            colors,
        ));
    }

    column.into()
}

fn plugin_list<'a>(store: &'a MarketPluginState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let tags: Vec<String> = PLUGIN_CATEGORIES
        .iter()
        .filter(|(key, _)| store.plugins.iter().any(|p| plugin_kind(p) == *key))
        .map(|(_, label)| label.to_string())
        .collect();

    let plugins: Vec<&MarketPlugin> = store
        .plugins
        .iter()
        .filter(|p| {
            store
                .selected_tag
                .as_ref()
                .map(|t| plugin_category_label(plugin_kind(p)) == *t)
                .unwrap_or(true)
        })
        .collect();

    if plugins.is_empty() {
        return center_msg(colors, "没有匹配的插件");
    }

    let mut list = column![].spacing(8).width(Length::Fill);
    if !tags.is_empty() {
        list = list.push(chip_bar(tags, &store.selected_tag, colors));
    }
    for plugin in plugins {
        let installed = store.installed.iter().find(|r| r.id == plugin.id).cloned();
        let is_downloading = store.downloading.as_deref() == Some(plugin.id.as_str());
        let is_installing = store.installing.as_deref() == Some(plugin.id.as_str());
        let progress = if is_downloading {
            store.download_progress
        } else {
            None
        };
        let downloaded = store.downloaded_ids.contains(&plugin.id);
        list = list.push(plugin_card(
            plugin,
            installed,
            downloaded,
            is_downloading,
            is_installing,
            progress,
            colors,
        ));
    }
    list.into()
}

/// 插件类型：优先 pluginType 字段（新索引），兼容旧 type 字段。
fn plugin_kind(plugin: &MarketPlugin) -> &str {
    let kind = plugin.plugin_kind.trim();
    if !kind.is_empty() {
        kind
    } else {
        &plugin.plugin_type
    }
}

fn plugin_category_label(kind: &str) -> String {
    PLUGIN_CATEGORIES
        .iter()
        .find(|(key, _)| *key == kind)
        .map(|(_, label)| label.to_string())
        .unwrap_or_else(|| kind.to_string())
}

fn plugin_card<'a>(
    plugin: &'a MarketPlugin,
    installed: Option<xime_plugin::PluginRecord>,
    downloaded: bool,
    downloading: bool,
    installing: bool,
    progress: Option<f32>,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let version = plugin
        .current_version
        .clone()
        .or_else(|| plugin.versions.first().map(|v| v.version.clone()))
        .unwrap_or_default();

    let kind = plugin_kind(plugin);
    let kind_label = plugin_category_label(kind);
    let glyph = kind_label.chars().next().unwrap_or('插');

    let mut meta = String::new();
    if !plugin.author.is_empty() {
        meta.push_str(&format!("作者：{}", plugin.author));
    }
    if !kind_label.is_empty() {
        if !meta.is_empty() {
            meta.push('　');
        }
        meta.push_str(&kind_label);
    }
    if !version.is_empty() {
        if !meta.is_empty() {
            meta.push('　');
        }
        meta.push_str(&format!("v{version}"));
    }

    let installed_version = installed
        .as_ref()
        .map(|r| r.version.clone())
        .unwrap_or_default();
    let is_installed = installed.is_some();
    let enabled = installed.as_ref().map(|r| r.enabled).unwrap_or(false);

    let mut body = column![row![
        glyph_box(glyph, colors),
        column![
            text(if plugin.name.is_empty() {
                plugin.id.clone()
            } else {
                plugin.name.clone()
            })
            .size(15)
            .font(semibold())
            .color(colors.foreground),
            if plugin.description.is_empty() {
                text("").size(12)
            } else {
                text(plugin.description.to_string())
                    .size(13)
                    .color(colors.foreground_muted)
            },
            if meta.is_empty() {
                text("").size(12)
            } else {
                text(meta).size(12).color(colors.foreground_muted)
            },
        ]
        .spacing(3)
        .width(Length::Fill),
        plugin_action(
            plugin.id.clone(),
            is_installed,
            downloaded,
            downloading,
            installing,
            progress,
            colors,
        ),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .width(Length::Fill),]
    .spacing(8)
    .width(Length::Fill);

    if is_installed {
        let row = row![
            switch(enabled, colors, move |on| {
                Message::TogglePlugin(plugin.id.clone(), on)
            }),
            text(if enabled { "已启用" } else { "已禁用" })
                .size(12)
                .color(colors.foreground_muted),
            container(text("")).width(Length::Fill),
            if !installed_version.is_empty() {
                text(format!("已安装 v{installed_version}"))
                    .size(12)
                    .color(colors.foreground_muted)
            } else {
                text("已安装").size(12).color(colors.foreground_muted)
            },
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill);
        body = body.push(row);
    }

    container(body)
        .width(Length::Fill)
        .padding(14)
        .style(move |_| card_style(colors))
        .into()
}

fn plugin_action<'a>(
    plugin_id: String,
    installed: bool,
    downloaded: bool,
    downloading: bool,
    installing: bool,
    progress: Option<f32>,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let colors = *colors;
    if installing {
        disabled_button("安装中…", colors)
    } else if downloading {
        let label = match progress {
            Some(p) => format!("下载中 {:.0}%", p * 100.0),
            None => "下载中…".to_string(),
        };
        disabled_button(label, colors)
    } else if installed {
        button_danger("卸载", &colors, Message::UninstallPlugin(plugin_id)).into()
    } else if downloaded {
        button_primary("安装", &colors, Message::InstallPlugin(plugin_id)).into()
    } else {
        button_primary("获取", &colors, Message::DownloadPlugin(plugin_id)).into()
    }
}

// ---- 公共小组件 ----

fn center_msg<'a>(colors: &'a ThemeColors, msg: &'a str) -> Element<'a, Message> {
    container(text(msg).size(14).color(colors.foreground_muted))
        .width(Length::Fill)
        .padding(32)
        .center_x(Length::Fill)
        .into()
}

fn error_view<'a>(colors: &'a ThemeColors, error: &'a str) -> Element<'a, Message> {
    column![
        text(error.to_string())
            .size(14)
            .color(colors.foreground_muted),
        text_button("重试", colors, Message::MarketRetry),
    ]
    .spacing(12)
    .align_x(Alignment::Start)
    .width(Length::Fill)
    .padding(24)
    .into()
}

fn footer<'a>(
    count: usize,
    kind: &str,
    updated_at: &'a str,
    colors: &'a ThemeColors,
) -> Element<'a, Message> {
    let mut text_ = format!("共 {} 个{}", count, kind);
    if !updated_at.is_empty() {
        text_ = format!("{} · 更新于 {}", text_, updated_at);
    }
    text(text_).size(11).color(colors.foreground_faint).into()
}

/// 彩色方块：分类字 + 主色浅底（商店卡片图标位）。
fn glyph_box<'a>(glyph: char, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    container(
        text(glyph.to_string())
            .size(16)
            .font(semibold())
            .color(colors.primary),
    )
    .width(40)
    .height(40)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .style(move |_| container::Style {
        background: Some(Background::Color(colors.primary_dim)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: border::radius(10.0),
        },
        ..container::Style::default()
    })
    .into()
}

/// 版本选择：多版本用下拉，单版本直接展示。
fn version_selector<'a>(
    id: String,
    versions: Vec<String>,
    selected: String,
    colors: &'a ThemeColors,
    on_select: fn(String, String) -> Message,
) -> Element<'a, Message> {
    if versions.is_empty() {
        return text("").size(12).into();
    }
    if versions.len() <= 1 {
        return text(selected).size(12).color(colors.primary).into();
    }
    let colors = *colors;
    pick_list(versions, Some(selected), move |v| on_select(id.clone(), v))
        .placeholder("版本")
        .padding([4, 8])
        .text_size(12)
        .width(Length::Shrink)
        .style(move |_theme, _status| iced::widget::pick_list::Style {
            text_color: colors.primary,
            background: Background::Color(colors.surface_variant),
            border: Border {
                color: colors.border,
                width: 1.0,
                radius: border::radius(8.0),
            },
            handle_color: colors.primary,
            placeholder_color: colors.foreground_muted,
        })
        .into()
}

fn disabled_button<'a>(label: impl Into<String>, colors: ThemeColors) -> Element<'a, Message> {
    button(text(label.into()).size(13).color(colors.foreground_muted))
        .padding([6, 14])
        .style(move |_theme, _status| {
            crate::components::widgets::secondary_button_style(&colors, button::Status::Disabled)
        })
        .into()
}

fn pill<'a>(label: &str, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
    container(
        text(label.to_string())
            .size(11)
            .color(colors.foreground_muted),
    )
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

fn warning_box<'a>(warning: &str, colors: &'a ThemeColors) -> Element<'a, Message> {
    let colors = *colors;
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
        })
        .into()
}

pub(crate) fn format_size(s: &str) -> String {
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
