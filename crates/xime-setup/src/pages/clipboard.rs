#![cfg(feature = "clipboard-page")]
use crate::components::{SettingsControl, SettingsGroup, SettingsItem, SettingsPage};
use crate::state::SettingsState;
use gpui::*;

pub fn render(
    settings: Entity<SettingsState>,
    cx: &mut Context<SettingsState>,
) -> impl IntoElement {
    let colors = cx.read_entity(&settings, |state, _| state.colors());

    SettingsPage::new("剪贴板", colors.clone())
        .group(
            SettingsGroup::new("剪贴板历史", colors.clone())
                .description("管理剪贴板历史记录")
                .items(vec![SettingsItem::new(
                    "启用剪贴板历史",
                    SettingsControl::label("开发中"),
                )
                .description("记录复制历史以便快速粘贴")]),
        )
        .group(SettingsGroup::new("操作", colors.clone()).items(vec![
            SettingsItem::button("清空历史").on_click({
                let settings = settings.clone();
                move |_window, cx| {
                    cx.update_entity(&settings, |state, cx| {
                        state.deploy_message = Some("功能开发中".to_string());
                        cx.notify();
                    });
                }
            }),
        ]))
}
