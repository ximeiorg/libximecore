use crate::components::{NumberInput, SettingsControl, SettingsGroup, SettingsItem, SettingsPage};
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
        state.load_schemas(cx);
        state.load_schema_config(cx);
    });

    let state = settings.read(cx);
    let schemas = &state.input_schema.available_schemas;
    let selected = state.input_schema.selected_schema;
    let config = &state.input_schema.schema_config;

    let current_schema_name = schemas
        .get(selected)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "未选择".to_string());

    let has_prev = selected > 0;
    let has_next = selected + 1 < schemas.len();

    SettingsPage::new("输入方案", colors.clone())
        .group(
            SettingsGroup::new("方案选择", colors.clone())
                .description("选择需要配置的输入方案")
                .custom_item(schema_selector(
                    colors,
                    &current_schema_name,
                    has_prev,
                    has_next,
                    settings.clone(),
                )),
        )
        .group(
            SettingsGroup::new("拼写设置", colors.clone())
                .description("拼写相关配置")
                .items(vec![
                    SettingsItem::new(
                        "最大编码长度",
                        SettingsControl::NumberInput(
                            NumberInput::new(config.speller.max_code_length.unwrap_or(0) as f64)
                                .min(1.0)
                                .max(99.0)
                                .on_change({
                                    let settings = settings.clone();
                                    move |value, _window, cx| {
                                        cx.update_entity(&settings, |state, cx| {
                                            state.input_schema.schema_config
                                                .speller.max_code_length =
                                                Some(value as i32);
                                            cx.notify();
                                        });
                                    }
                                }),
                        ),
                    )
                    .description("最大编码长度"),
                    SettingsItem::new(
                        "自动上屏",
                        SettingsControl::switch_with(
                            config.speller.auto_select.unwrap_or(false),
                            {
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.input_schema.schema_config.speller.auto_select =
                                            Some(value);
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .description("唯一候选时自动上屏"),
                ]),
        )
        .group(
            SettingsGroup::new("翻译器设置", colors.clone())
                .description("翻译相关配置")
                .items(vec![
                    SettingsItem::new(
                        "字符集过滤",
                        SettingsControl::switch_with(
                            config.translator.enable_charset_filter.unwrap_or(false),
                            {
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.input_schema.schema_config.translator
                                            .enable_charset_filter = Some(value);
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .description("启用字符集过滤"),
                    SettingsItem::new(
                        "补全",
                        SettingsControl::switch_with(
                            config.translator.enable_completion.unwrap_or(false),
                            {
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.input_schema.schema_config.translator
                                            .enable_completion = Some(value);
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .description("启用补全功能"),
                    SettingsItem::new(
                        "造句",
                        SettingsControl::switch_with(
                            config.translator.enable_sentence.unwrap_or(false),
                            {
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.input_schema.schema_config.translator
                                            .enable_sentence = Some(value);
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .description("启用造句功能"),
                    SettingsItem::new(
                        "用户词典",
                        SettingsControl::switch_with(
                            config.translator.enable_user_dict.unwrap_or(false),
                            {
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.input_schema.schema_config.translator
                                            .enable_user_dict = Some(value);
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .description("启用用户词典"),
                    SettingsItem::new(
                        "编码器",
                        SettingsControl::switch_with(
                            config.translator.enable_encoder.unwrap_or(false),
                            {
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.input_schema.schema_config.translator
                                            .enable_encoder = Some(value);
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .description("启用编码器"),
                    SettingsItem::new(
                        "编码历史",
                        SettingsControl::switch_with(
                            config.translator.encode_commit_history.unwrap_or(false),
                            {
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.input_schema.schema_config.translator
                                            .encode_commit_history = Some(value);
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .description("记录编码历史"),
                    SettingsItem::new(
                        "最大短语长度",
                        SettingsControl::NumberInput(
                            NumberInput::new(
                                config.translator.max_phrase_length.unwrap_or(0) as f64,
                            )
                            .min(0.0)
                            .max(100.0)
                            .on_change({
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.input_schema.schema_config.translator
                                            .max_phrase_length = Some(value as i32);
                                        cx.notify();
                                    });
                                }
                            }),
                        ),
                    )
                    .description("最大短语长度"),
                ]),
        )
        .group(
            SettingsGroup::new("操作", colors.clone())
                .description("保存并应用配置")
                .items(vec![
                    SettingsItem::new(
                        "保存方案",
                        SettingsControl::button_with("保存方案", {
                            let settings = settings.clone();
                            move |_window, cx| {
                                cx.update_entity(&settings, |state, cx| {
                                    if let Err(e) = state.save_schema() {
                                        state.deploy_message =
                                            Some(format!("保存失败: {}", e));
                                        cx.notify();
                                    } else {
                                        state.deploy_message =
                                            Some("方案已保存".to_string());
                                        cx.notify();
                                    }
                                });
                            }
                        }),
                    ),
                    SettingsItem::new(
                        "保存配置",
                        SettingsControl::button_with("保存配置", {
                            let settings = settings.clone();
                            move |_window, cx| {
                                cx.update_entity(&settings, |state, cx| {
                                    if let Err(e) = state.save_schema_config() {
                                        state.deploy_message =
                                            Some(format!("保存配置失败: {}", e));
                                        cx.notify();
                                    } else {
                                        state.deploy_message =
                                            Some("配置已保存".to_string());
                                        cx.notify();
                                    }
                                });
                            }
                        }),
                    ),
                    SettingsItem::new(
                        "重新部署",
                        SettingsControl::button_with("重新部署", {
                            let settings = settings.clone();
                            move |_window, cx| {
                                cx.update_entity(&settings, |state, cx| {
                                    if let Err(e) = state.deploy() {
                                        state.deploy_message =
                                            Some(format!("部署失败: {}", e));
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                    ),
                ]),
        )
}

fn schema_selector(
    colors: &ThemeColors,
    current_name: &str,
    has_prev: bool,
    has_next: bool,
    settings: Entity<SettingsState>,
) -> Div {
    SettingsItem::render_custom(
        colors,
        "当前方案",
        Some("选择要配置的输入方案"),
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id("prev-schema")
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(colors.primary)
                    .text_color(colors.on_primary)
                    .text_size(px(14.0))
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.8))
                    .when(!has_prev, |this| this.opacity(0.3))
                    .on_click({
                        let settings = settings.clone();
                        move |_, _window, cx| {
                            cx.update_entity(&settings, |state, cx| {
                                if state.input_schema.selected_schema > 0 {
                                    state.input_schema.selected_schema -= 1;
                                    state.input_schema.config_loaded = false;
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .child("◀"),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(colors.foreground)
                    .px(px(12.0))
                    .child(current_name.to_string()),
            )
            .child(
                div()
                    .id("next-schema")
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(colors.primary)
                    .text_color(colors.on_primary)
                    .text_size(px(14.0))
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.8))
                    .when(!has_next, |this| this.opacity(0.3))
                    .on_click({
                        let settings = settings.clone();
                        move |_, _window, cx| {
                            cx.update_entity(&settings, |state, cx| {
                                if state.input_schema.selected_schema + 1
                                    < state.input_schema.available_schemas.len()
                                {
                                    state.input_schema.selected_schema += 1;
                                    state.input_schema.config_loaded = false;
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .child("▶"),
            ),
    )
}
