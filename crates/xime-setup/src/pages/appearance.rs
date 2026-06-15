use crate::components::{SettingsControl, SettingsGroup, SettingsItem, SettingsPage};
use crate::state::SettingsState;
use crate::theme::ThemeColors;
use gpui::*;

pub fn render(settings: &Entity<SettingsState>, colors: &ThemeColors) -> impl IntoElement {
    SettingsPage::new("外观", colors.clone())
        .group(SettingsGroup::new("显示", colors.clone()).items(vec![
                    SettingsItem::new(
                        "字号",
                        SettingsControl::number_input_with(
                            14.0,
                            {
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.appearance.font_size = value;
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .description("候选词显示字号"),
                    SettingsItem::new(
                        "候选词数量",
                        SettingsControl::number_input_with(
                            5.0,
                            {
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.appearance.candidate_count = value as i32;
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .description("候选词列表中显示的数量"),
                    SettingsItem::new(
                        "圆角大小",
                        SettingsControl::number_input_with(
                            8.0,
                            {
                                let settings = settings.clone();
                                move |value, _window, cx| {
                                    cx.update_entity(&settings, |state, cx| {
                                        state.appearance.corner_radius = value;
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    )
                    .description("候选窗口圆角半径"),
                ]))
        .group(
            SettingsGroup::new("操作", colors.clone()).items(vec![SettingsItem::new(
                "保存外观设置",
                SettingsControl::button_with("保存外观设置", {
                    let settings = settings.clone();
                    move |_window, cx| {
                        cx.update_entity(&settings, |state, cx| {
                            if let Err(e) = state.save_appearance() {
                                state.deploy_message = Some(format!("保存失败: {}", e));
                                cx.notify();
                            } else {
                                state.deploy_message = Some("外观设置已保存并重载".to_string());
                                cx.notify();
                            }
                        });
                    }
                }),
            )]),
        )
}
