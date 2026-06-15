#![cfg(feature = "pair-page")]
use crate::components::{SettingsControl, SettingsGroup, SettingsItem, SettingsPage};
use crate::state::SettingsState;
use gpui::*;

pub fn render(
    settings: Entity<SettingsState>,
    cx: &mut Context<SettingsState>,
) -> impl IntoElement {
    let colors = cx.read_entity(&settings, |state, _| state.colors());

    SettingsPage::new("设备关联", colors.clone())
        .group(
            SettingsGroup::new("设备配对", colors.clone())
                .description("通过配对码关联多台设备")
                .items(vec![SettingsItem::new(
                    "配对状态",
                    SettingsControl::label("未配对"),
                )
                .description("当前设备未关联到任何账户")]),
        )
        .group(SettingsGroup::new("操作", colors.clone()).items(vec![
            SettingsItem::button("开始配对").on_click({
                let settings = settings.clone();
                move |_window, cx| {
                    cx.update_entity(&settings, |state, cx| {
                        // TODO: implement pairing
                        state.deploy_message = Some("功能开发中".to_string());
                        cx.notify();
                    });
                }
            }),
        ]))
}
