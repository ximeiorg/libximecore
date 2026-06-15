pub mod about;
pub mod appearance;
pub mod dictionary;
pub mod hotkeys;
pub mod input_schema;

#[cfg(feature = "smart-suggestion-page")]
pub mod smart_suggestion;
#[cfg(feature = "pair-page")]
pub mod pair;
#[cfg(feature = "clipboard-page")]
pub mod clipboard;
#[cfg(target_os = "linux")]
pub mod sync;

use crate::components::TitleBar;
use crate::state::SettingsState;
use gpui::{prelude::FluentBuilder, IntoElement, ParentElement, *};

pub struct SettingsApp {
    current_page: usize,
    pub settings: Entity<SettingsState>,
}

impl SettingsApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = cx.new(SettingsState::new);
        Self {
            current_page: 0,
            settings,
        }
    }

    fn sidebar_items() -> Vec<(&'static str, &'static str)> {
        let mut items = vec![
            ("icons/keyboard.svg", "输入方案"),
            ("icons/palette.svg", "外观"),
            ("icons/command.svg", "快捷键"),
            ("icons/books.svg", "词典"),
        ];

        #[cfg(feature = "smart-suggestion-page")]
        items.push(("icons/thinking.svg", "智能联想"));

        #[cfg(target_os = "linux")]
        items.push(("icons/sync.svg", "同步"));

        #[cfg(feature = "pair-page")]
        items.push(("icons/link.svg", "设备关联"));

        #[cfg(feature = "clipboard-page")]
        items.push(("icons/clipboard.svg", "剪贴板"));

        items.push(("icons/about.svg", "关于"));
        items
    }
}

impl Render for SettingsApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_background_appearance(WindowBackgroundAppearance::Blurred);

        let pages = Self::sidebar_items();
        let page_count = pages.len();
        let current = self.current_page.min(page_count.saturating_sub(1));
        let settings = self.settings.clone();
        let settings_for_title = settings.clone();
        let colors = cx.read_entity(&settings, |state, _| state.colors());

        let sidebar = div()
            .w(px(213.0))
            .min_w(px(213.0))
            .max_w(px(213.0))
            .h_full()
            .bg(colors.sidebar_bg)
            .flex()
            .flex_col()
            .gap(px(2.0))
            .p(px(8.0))
            .children(pages.iter().enumerate().map(|(i, (icon_path, name))| {
                let is_current = i == current;
                let view = cx.entity();
                div()
                    .id(("menu", i))
                    .py(px(10.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .when(is_current, |this: Stateful<Div>| this.bg(colors.primary))
                    .when(!is_current, |this: Stateful<Div>| {
                        this.cursor_pointer()
                            .hover(|style: StyleRefinement| style.bg(hsla(0.0, 0.0, 1.0, 0.15)))
                    })
                    .text_size(px(15.0))
                    .text_color(colors.on_primary)
                    .on_click(move |_, _window: &mut Window, cx: &mut App| {
                        cx.update_entity(
                            &view,
                            |app: &mut SettingsApp, cx: &mut Context<SettingsApp>| {
                                app.current_page = i;
                                cx.notify();
                            },
                        );
                    })
                    .child(img(*icon_path).w(px(20.0)).h(px(20.0)))
                    .child(
                        div()
                            .text_size(px(15.0))
                            .text_color(colors.on_primary)
                            .child(name.to_string()),
                    )
            }));

        let mut page_offset = 4;
        let content: AnyElement = match current {
            0 => input_schema::render(&settings, &colors).into_any_element(),
            1 => appearance::render(&settings, &colors).into_any_element(),
            2 => hotkeys::render(&settings, &colors, cx).into_any_element(),
            3 => dictionary::render(&settings, &colors).into_any_element(),
            i if i == page_offset && cfg!(feature = "smart-suggestion-page") => {
                #[cfg(feature = "smart-suggestion-page")]
                {
                    page_offset += 1;
                    smart_suggestion::render(&settings, &colors).into_any_element()
                }
                #[cfg(not(feature = "smart-suggestion-page"))]
                {
                    page_offset += 0;
                    about::render(&settings, &colors).into_any_element()
                }
            }
            i if i == page_offset && cfg!(target_os = "linux") => {
                #[cfg(target_os = "linux")]
                {
                    page_offset += 1;
                    sync::render(&settings, &colors).into_any_element()
                }
                #[cfg(not(target_os = "linux"))]
                {
                    page_offset += 0;
                    about::render(&settings, &colors).into_any_element()
                }
            }
            i if i == page_offset && cfg!(feature = "pair-page") => {
                #[cfg(feature = "pair-page")]
                {
                    page_offset += 1;
                    pair::render(&settings, &colors).into_any_element()
                }
                #[cfg(not(feature = "pair-page"))]
                {
                    page_offset += 0;
                    about::render(&settings, &colors).into_any_element()
                }
            }
            i if i == page_offset && cfg!(feature = "clipboard-page") => {
                #[cfg(feature = "clipboard-page")]
                {
                    clipboard::render(&settings, &colors).into_any_element()
                }
                #[cfg(not(feature = "clipboard-page"))]
                {
                    about::render(&settings, &colors).into_any_element()
                }
            }
            _ => about::render(&settings, &colors).into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(TitleBar::render(&settings_for_title, &colors))
            .child(
                div()
                    .id("content-area")
                    .flex()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .child(sidebar)
                    .child(
                        div()
                            .id("content-scroll")
                            .flex_1()
                            .min_w(px(400.0))
                            .overflow_y_scroll()
                            .bg(colors.background)
                            .child(content),
                    ),
            )
            .into_any_element()
    }
}
