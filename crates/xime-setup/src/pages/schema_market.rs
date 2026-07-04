use crate::components::{SettingsGroup, SettingsPage};
use crate::pages::SettingsApp;
use crate::state::{MarketSchema, SettingsState};
use crate::theme::ThemeColors;
use gpui::prelude::FluentBuilder;
use gpui::*;

pub fn render(
    settings: &Entity<SettingsState>,
    colors: &ThemeColors,
    cx: &mut Context<SettingsApp>,
) -> impl IntoElement {
    cx.update_entity(settings, |state, cx| {
        state.check_market_task_result(cx);
        state.apply_market_yaml(cx);
        state.load_market_schemas(cx);
    });

    let state = settings.read(cx);
    let market = &state.market_schema;

    SettingsPage::new("方案市场", colors.clone()).group(
        SettingsGroup::new("方案列表", colors.clone())
            .description("从方案市场下载并安装输入方案")
            .custom_item(if market.loading && !market.loaded {
                center_msg(colors, "正在加载方案列表…")
            } else if let Some(ref error) = market.error {
                error_view(colors, error, settings.clone())
            } else if market.loaded {
                schema_list(colors, market, settings.clone())
            } else {
                center_msg(colors, "正在加载方案列表…")
            }),
    )
}

fn center_msg(colors: &ThemeColors, msg: &str) -> Div {
    div()
        .py(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(px(14.0))
                .text_color(colors.foreground_muted)
                .child(msg.to_string()),
    )
}

fn error_view(colors: &ThemeColors, error: &str, settings: Entity<SettingsState>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .py(px(24.0))
        .px(px(16.0))
        .child(
            div()
                .text_size(px(14.0))
                .text_color(colors.foreground_muted)
                .child(error.to_string()),
        )
        .child(
            div()
                .id("retry-btn")
                .py(px(8.0))
                .px(px(16.0))
                .rounded(px(8.0))
                .bg(colors.primary)
                .text_color(colors.on_primary)
                .text_size(px(14.0))
                .cursor_pointer()
                .child("重试")
                .on_click(move |_, _window, cx| {
                    cx.update_entity(&settings, |state, cx| {
                        state.market_schema.loading = false;
                        state.market_schema.error = None;
                        state.market_schema.loaded = false;
                        cx.notify();
                    });
                }),
        )
}

fn schema_list(
    colors: &ThemeColors,
    market: &crate::state::MarketSchemaState,
    settings: Entity<SettingsState>,
) -> Div {
    let mut items: Vec<Div> = Vec::new();
    let schemas = market.schemas.clone();
    let installed = &market.installed_ids;
    let downloaded = &market.downloaded_ids;
    let downloading = market.downloading.as_deref();
    let installing = market.installing.as_deref();
    let _msg = market.install_message.as_deref();

    for (i, schema) in schemas.iter().enumerate() {
        let is_installed = installed.contains(&schema.id);
        let is_downloaded = downloaded.contains(&schema.id);
        let is_downloading = downloading == Some(schema.id.as_str());
        let is_installing = installing == Some(schema.id.as_str());

        items.push(card(
            colors,
            schema,
            i as u64,
            is_installed,
            is_downloaded,
            is_downloading,
            is_installing,
            _msg,
            settings.clone(),
        ));
    }

    div().flex().flex_col().gap(px(8.0)).children(items)
}

fn card(
    colors: &ThemeColors,
    schema: &MarketSchema,
    idx: u64,
    installed: bool,
    downloaded: bool,
    downloading: bool,
    installing: bool,
    _msg: Option<&str>,
    settings: Entity<SettingsState>,
) -> Div {
    let version = schema
        .current_version
        .as_deref()
        .unwrap_or("latest");

    let size_label = schema
        .versions
        .iter()
        .find(|v| v.version == version || (schema.current_version.is_none() && v.version == "latest"))
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

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(14.0))
        .rounded(px(12.0))
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.foreground)
                        .child(if schema.name.is_empty() {
                            schema.id.clone()
                        } else {
                            schema.name.clone()
                        }),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors.primary)
                        .child(version.to_string()),
                )
                .when(schema.schema_type == "built-in", |this| {
                    this.child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .text_size(px(11.0))
                            .bg(colors.primary)
                            .text_color(colors.on_primary)
                            .child("内置"),
                    )
                }),
        )
        .when(!schema.description.is_empty(), |this| {
            this.child(
                div()
                    .text_size(px(13.0))
                    .text_color(colors.foreground_muted)
                    .max_w(px(500.0))
                    .child(schema.description.clone()),
            )
        })
        .child(
            div()
                .text_size(px(12.0))
                .text_color(colors.foreground_muted)
                .child(if schema.author.is_empty() {
                    schema.tags.join("、")
                } else {
                    format!("作者：{}　{}", schema.author, schema.tags.join("、"))
                }),
        )
        .when(!deps.is_empty(), |this| {
            this.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.0))
                    .children(deps.iter().take(4).map(|dep| {
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .text_size(px(11.0))
                            .bg(colors.surface_variant)
                            .text_color(colors.foreground_muted)
                            .child(dep.clone())
                    })),
            )
        })
        .when_some(schema.warning.clone(), |this, warning| {
            if warning.is_empty() {
                return this;
            }
            this.child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .text_size(px(12.0))
                    .bg(hsla(0.0, 0.5, 0.2, 0.3))
                    .text_color(colors.foreground)
                    .child(warning),
            )
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors.foreground_muted)
                        .child(if !size_label.is_empty() {
                            format!("大小: {}", size_label)
                        } else {
                            String::new()
                        }),
                )
                .child(action_button(
                    colors,
                    idx,
                    schema.id.clone(),
                    installed,
                    downloaded,
                    downloading,
                    installing,
            _msg,
                    settings,
                )),
        )
}

fn action_button(
    colors: &ThemeColors,
    idx: u64,
    schema_id: String,
    installed: bool,
    downloaded: bool,
    downloading: bool,
    installing: bool,
    _msg: Option<&str>,
    settings: Entity<SettingsState>,
) -> impl IntoElement {
    if installing {
        div()
            .id(("market-btn", idx))
            .py(px(6.0))
            .px(px(12.0))
            .rounded(px(8.0))
            .bg(colors.surface_variant)
            .text_color(colors.foreground_muted)
            .text_size(px(13.0))
            .child("安装中…")
            .into_any_element()
    } else if downloading {
        div()
            .id(("market-btn", idx))
            .py(px(6.0))
            .px(px(12.0))
            .rounded(px(8.0))
            .bg(colors.surface_variant)
            .text_color(colors.foreground_muted)
            .text_size(px(13.0))
            .child("下载中…")
            .into_any_element()
    } else if installed {
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id(("market-deploy", idx))
                    .py(px(6.0))
                    .px(px(14.0))
                    .rounded(px(8.0))
                    .bg(colors.primary)
                    .text_color(colors.on_primary)
                    .text_size(px(13.0))
                    .cursor_pointer()
                    .child("部署")
                    .on_click({
                        let settings = settings.clone();
                        move |_, _window, cx| {
                            cx.update_entity(&settings, |state, cx| {
                                if let Err(e) = state.deploy() {
                                    state.market_schema.install_message =
                                        Some(format!("部署失败: {}", e));
                                }
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(colors.foreground_muted)
                    .child("已安装"),
            )
            .into_any_element()
    } else if downloaded {
        div()
            .id(("market-install", idx))
            .py(px(6.0))
            .px(px(14.0))
            .rounded(px(8.0))
            .bg(colors.primary)
            .text_color(colors.on_primary)
            .text_size(px(13.0))
            .cursor_pointer()
            .child("安装")
            .on_click({
                let settings = settings.clone();
                let sid = schema_id.clone();
                move |_, _window, cx| {
                    cx.update_entity(&settings, |state, cx| {
                        state.install_market_schema(&sid, cx);
                    });
                }
            })
            .into_any_element()
    } else {
        div()
            .id(("market-download", idx))
            .py(px(6.0))
            .px(px(14.0))
            .rounded(px(8.0))
            .bg(colors.primary)
            .text_color(colors.on_primary)
            .text_size(px(13.0))
            .cursor_pointer()
            .child("下载")
            .on_click({
                let settings = settings.clone();
                let sid = schema_id.clone();
                move |_, _window, cx| {
                    cx.update_entity(&settings, |state, cx| {
                        state.download_market_schema(&sid, cx);
                    });
                }
            })
            .into_any_element()
    }
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
