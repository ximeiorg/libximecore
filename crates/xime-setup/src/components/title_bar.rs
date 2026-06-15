use crate::state::SettingsState;
use crate::theme::ThemeColors;
use gpui::*;

pub struct TitleBar;

impl TitleBar {
    pub fn render(_settings: &Entity<SettingsState>, colors: &ThemeColors) -> impl IntoElement {
        div()
            .w_full()
            .h(px(52.0))
            .flex()
            .items_center()
            .px(px(16.0))
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        img("icons/xime.svg")
                            .w(px(24.0))
                            .h(px(24.0))
                            .rounded(px(6.0)),
                    )
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.foreground)
                            .child("Xime 设置"),
                    ),
            )
    }
}
