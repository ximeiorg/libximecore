use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{button_primary, kbd};
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::row;
use iced::Element;

pub fn view<'a>(_settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let display_name = xime_config::app_metadata().display_name;
    settings_page(
        "快捷键",
        colors,
        vec![
            settings_group(
                "常用快捷键",
                Some(format!("{display_name} 输入法快捷键配置")),
                colors,
                vec![
                    settings_item(
                        "中/英切换",
                        Some("切换中文/英文输入模式"),
                        colors,
                        kbd("Shift", colors),
                    ),
                    settings_item(
                        "中/英切换",
                        Some("切换中文/英文输入模式（备选）"),
                        colors,
                        kbd("Ctrl+Space", colors),
                    ),
                    settings_item(
                        "全/半角切换",
                        Some("切换全角/半角符号"),
                        colors,
                        kbd("Ctrl+.", colors),
                    ),
                    settings_item(
                        "中/英标点切换",
                        Some("切换中文/英文标点"),
                        colors,
                        kbd("Ctrl+,", colors),
                    ),
                ],
            ),
            settings_group(
                "候选词选择",
                Some("候选词翻页和选择"),
                colors,
                vec![
                    settings_item("下一页", Some("候选词翻到下一页"), colors, kbd("[", colors)),
                    settings_item("上一页", Some("候选词翻到上一页"), colors, kbd("]", colors)),
                ],
            ),
            settings_group(
                "操作",
                None::<String>,
                colors,
                vec![
                    settings_item(
                        "显示字根",
                        Some("按住 Ctrl 键显示当前按键对应的五笔字根"),
                        colors,
                        kbd("Ctrl", colors),
                    ),
                    row![button_primary("重新部署", colors, Message::DeploySchemas)].into(),
                ],
            ),
        ],
    )
}
