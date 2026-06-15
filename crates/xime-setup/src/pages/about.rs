use crate::components::{SettingsGroup, SettingsPage};
use crate::state::SettingsState;
use crate::theme::ThemeColors;
use gpui::*;

pub fn render(
    _settings: &Entity<SettingsState>,
    colors: &ThemeColors,
) -> impl IntoElement {
    SettingsPage::new("关于 Xime", colors.clone())
        .group(
            SettingsGroup::new("Xime 输入法", colors.clone())
                .items(vec![])
                .custom_item(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(16.0))
                        .pb(px(16.0))
                        .child(
                            img("icons/xime.svg")
                                .w(px(64.0))
                                .h(px(64.0))
                                .rounded(px(16.0)),
                        )
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.foreground)
                                .child("Xime"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors.foreground_muted)
                                .child("版本 0.2.0"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors.foreground_muted)
                                .child("基于 Rime 引擎的五笔输入法"),
                        )
                        .child(
                            div()
                                .pt(px(16.0))
                                .text_size(px(12.0))
                                .text_color(colors.foreground_muted)
                                .child("使用 librime + GPUI 构建"),
                        ),
                ),
        )
}
