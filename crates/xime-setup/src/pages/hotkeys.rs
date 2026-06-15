use crate::components::{SettingsControl, SettingsGroup, SettingsItem, SettingsPage};
use crate::pages::SettingsApp;
use crate::state::SettingsState;
use crate::theme::ThemeColors;
use gpui::*;

pub fn render(
    settings: &Entity<SettingsState>,
    colors: &ThemeColors,
    _cx: &mut Context<SettingsApp>,
) -> impl IntoElement {
    SettingsPage::new("快捷键", colors.clone())
        .group(
            SettingsGroup::new("常用快捷键", colors.clone())
                .description("Xime 输入法快捷键配置")
                .items(vec![
                    SettingsItem::new("中/英切换", SettingsControl::kbd("Shift"))
                        .description("切换中文/英文输入模式"),
                    SettingsItem::new("中/英切换", SettingsControl::kbd("Ctrl+Space"))
                        .description("切换中文/英文输入模式（备选）"),
                    SettingsItem::new("全/半角切换", SettingsControl::kbd("Ctrl+."))
                        .description("切换全角/半角符号"),
                    SettingsItem::new("中/英标点切换", SettingsControl::kbd("Ctrl+,"))
                        .description("切换中文/英文标点"),
                ]),
        )
        .group(
            SettingsGroup::new("候选词选择", colors.clone())
                .description("候选词翻页和选择")
                .items(vec![
                    SettingsItem::new("下一页", SettingsControl::kbd("["))
                        .description("候选词翻到下一页"),
                    SettingsItem::new("上一页", SettingsControl::kbd("]"))
                        .description("候选词翻到上一页"),
                ]),
        )
        .group(SettingsGroup::new("操作", colors.clone()).items(vec![
                SettingsItem::new("显示字根", SettingsControl::kbd("Ctrl"))
                    .description("按住 Ctrl 键显示当前按键对应的五笔字根"),
                SettingsItem::new(
                    "重新部署",
                    SettingsControl::button_with("重新部署", {
                        let settings = settings.clone();
                        move |_window, cx| {
                            cx.update_entity(&settings, |state, cx| {
                                if let Err(e) = state.deploy() {
                                    state.deploy_message = Some(format!("部署失败: {}", e));
                                    cx.notify();
                                }
                            });
                        }
                    }),
                ),
            ]))
}
