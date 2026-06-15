#![cfg(feature = "smart-suggestion-page")]
use crate::components::{SettingsControl, SettingsGroup, SettingsItem, SettingsPage};
use crate::state::SettingsState;
use gpui::*;

pub fn render(
    settings: Entity<SettingsState>,
    cx: &mut Context<SettingsState>,
) -> impl IntoElement {
    let colors = cx.read_entity(&settings, |state, _| state.colors());

    SettingsPage::new("智能联想", colors.clone())
        .group(
            SettingsGroup::new("AI 智能联想", colors.clone())
                .description("基于 ONNX 模型的智能输入联想")
                .items(vec![
                    SettingsItem::new("启用智能联想", SettingsControl::label("开发中"))
                        .description("开启后输入时自动联想下一个词"),
                    SettingsItem::new("联想数量", SettingsControl::label("5"))
                        .description("每次显示的建议数量"),
                ]),
        )
        .group(SettingsGroup::new("操作", colors.clone()).items(vec![
            SettingsItem::button("保存设置").on_click({
                let settings = settings.clone();
                move |_window, cx| {
                    cx.update_entity(&settings, |state, cx| {
                        // TODO: implement save
                        state.deploy_message = Some("功能开发中".to_string());
                        cx.notify();
                    });
                }
            }),
        ]))
}
