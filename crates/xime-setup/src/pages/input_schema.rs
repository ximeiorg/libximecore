use crate::pages::SettingsApp;
use crate::state::SettingsState;
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
        state.load_schemas(cx);
        state.load_market_schemas(cx);
        state.apply_market_yaml(cx);
    });

    let state = settings.read(cx);
    let tab = state.input_schema.current_tab;
    let installed_ids = state.market_schema.installed_ids.clone();

    div()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .p(px(16.0))
        .w_full()
        .child(
            div()
                .text_size(px(20.0))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.foreground)
                .pb(px(8.0))
                .child("输入方案"),
        )
        .child(tab_bar(colors, tab, settings.clone()))
        .child(if tab == 0 {
            installed_tab(colors, settings.clone(), cx).into_any_element()
        } else {
            downloads_tab(colors, installed_ids, settings.clone()).into_any_element()
        })
}

fn tab_bar(colors: &ThemeColors, active: usize, settings: Entity<SettingsState>) -> Div {
    let labels = ["已安装", "已下载"];
    let mut bar = div().flex().border_b_1().border_color(colors.border);
    for (i, label) in labels.iter().enumerate() {
        let is_active = i == active;
        let settings = settings.clone();
        bar = bar.child(
            div()
                .id(("schema-tab", i as u64))
                .px(px(16.0))
                .py(px(8.0))
                .text_size(px(14.0))
                .text_color(if is_active {
                    colors.primary
                } else {
                    colors.foreground_muted
                })
                .border_b_2()
                .border_color(if is_active {
                    colors.primary
                } else {
                    colors.border
                })
                .cursor_pointer()
                .on_click(move |_, _window, cx| {
                    cx.update_entity(&settings, |state, cx| {
                        state.input_schema.current_tab = i;
                        cx.notify();
                    });
                })
                .child(label.to_string()),
        );
    }
    bar
}

fn installed_tab(
    colors: &ThemeColors,
    settings: Entity<SettingsState>,
    cx: &mut Context<SettingsApp>,
) -> Div {
    let state = settings.read(cx);
    let schemas = state.input_schema.available_schemas.clone();
    let selected = state.input_schema.selected_schema;

    if schemas.is_empty() {
        return div()
            .py(px(24.0))
            .text_center()
            .text_size(px(13.0))
            .text_color(colors.foreground_muted)
            .child("暂无已安装的方案");
    }

    let mut list = div().flex().flex_col().gap(px(4.0));
    for (i, schema) in schemas.iter().enumerate() {
        let is_current = i == selected;
        let row = div()
            .flex()
            .items_center()
            .justify_between()
            .p(px(12.0))
            .rounded(px(8.0))
            .bg(if is_current {
                colors.primary
            } else {
                colors.surface
            })
            .text_color(if is_current {
                colors.on_primary
            } else {
                colors.foreground
            })
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .child(if schema.name.is_empty() {
                                schema.schema_id.clone()
                            } else {
                                format!("{} ({})", schema.name, schema.schema_id)
                            }),
                    )
                    .when(is_current, |this| {
                        this.child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .text_size(px(11.0))
                                .bg(hsla(0.0, 0.0, 1.0, 0.2))
                                .child("当前"),
                        )
                    }),
            )
            .child(
                div()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .text_size(px(12.0))
                    .bg(if is_current {
                        colors.on_primary
                    } else {
                        colors.surface_variant
                    })
                    .text_color(if is_current {
                        colors.primary
                    } else {
                        colors.foreground_muted
                    })
                    .cursor_pointer()
                    .child("设置"),
            );

        list = list.child(
            div()
                .id(("schema-row", i as u64))
                .cursor_pointer()
                .child(row)
                .on_click({
                    let settings = settings.clone();
                    move |_, _window, cx| {
                        cx.update_entity(&settings, |state, cx| {
                            state.input_schema.selected_schema = i;
                            state.input_schema.config_loaded = false;
                            cx.notify();
                        });
                    }
                }),
        );
    }

    list.child(
        div()
            .flex()
            .justify_center()
            .mt(px(16.0))
            .child(
                div()
                    .id("deploy-wrapper")
                    .cursor_pointer()
                    .child(
                        div()
                            .id("deploy-btn")
                            .py(px(8.0))
                            .px(px(20.0))
                            .rounded(px(8.0))
                            .bg(colors.primary)
                            .text_color(colors.on_primary)
                            .text_size(px(14.0))
                            .child("部署方案"),
                    )
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
            ),
    )
}

fn downloads_tab(
    colors: &ThemeColors,
    installed_ids: Vec<String>,
    settings: Entity<SettingsState>,
) -> Div {
    let market_dir = if cfg!(debug_assertions) {
        let mut p = std::env::current_exe().unwrap_or_default();
        p.pop(); while !p.join("Cargo.toml").exists() && p.parent().is_some() { p.pop(); }
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
            if name_str.starts_with('.') { continue; }
            if entry.path().is_dir() {
                let has_archive = std::fs::read_dir(entry.path())
                    .map(|mut e| e.any(|e| {
                        e.ok().and_then(|e| e.file_name().to_str().map(|n|
                            n.ends_with(".zip") || n.ends_with(".tar.gz")
                        )).unwrap_or(false)
                    }))
                    .unwrap_or(false);
                if has_archive {
                    pkg_ids.push(name_str.to_string());
                }
            }
        }
    }

    if pkg_ids.is_empty() {
        return div()
            .py(px(48.0))
            .text_center()
            .text_size(px(14.0))
            .text_color(colors.foreground_muted)
            .child("暂无已下载的方案包")
            .child(
                div().mt(px(8.0))
                    .text_size(px(13.0))
                    .text_color(colors.foreground_muted)
                    .child("请前往「方案市场」下载"),
            );
    }

    let mut list = div().flex().flex_col().gap(px(8.0));
    for (i, pkg_id) in pkg_ids.iter().enumerate() {
        let is_installed = installed_ids.contains(pkg_id);
        let name = pkg_id.clone();
        list = list.child(package_card(colors, &name, i as u64, is_installed, settings.clone()));
    }
    list
}

fn package_card(
    colors: &ThemeColors,
    pkg_id: &str,
    idx: u64,
    installed: bool,
    settings: Entity<SettingsState>,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .p(px(12.0))
        .rounded(px(8.0))
        .bg(colors.surface)
        .border_1()
        .border_color(colors.border)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .text_color(colors.foreground)
                        .child(pkg_id.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors.foreground_muted)
                        .child(if installed { "已安装至输入法" } else { "已下载，未安装" }),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id(("pkg-action", idx))
                        .py(px(6.0))
                        .px(px(14.0))
                        .rounded(px(6.0))
                        .text_size(px(13.0))
                        .cursor_pointer()
                        .bg(if installed {
                            hsla(0.0, 0.6, 0.4, 0.8)
                        } else {
                            colors.primary
                        })
                        .text_color(colors.on_primary)
                        .child(if installed { "卸载" } else { "安装" })
                        .on_click({
                            let settings = settings.clone();
                            let sid = pkg_id.to_string();
                            move |_, _window, cx| {
                                let sid = sid.clone();
                                cx.update_entity(&settings, |state, cx| {
                                    if installed {
                                        state.uninstall_market_schema(&sid, cx);
                                    } else {
                                        state.install_market_schema(&sid, cx);
                                    }
                                });
                            }
                        }),
                ),
        )
}
