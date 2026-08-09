use crate::pages;
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{column, container, row};
use iced::{Background, Element, Length, Subscription, Task, Theme};

/// 设置应用根状态（iced Application 的 State）。
pub struct SettingsApp {
    pub settings: SettingsState,
    /// 当前主题颜色（由设置派生，视图与主题共用）。
    pub colors: ThemeColors,
}

impl SettingsApp {
    pub fn new() -> Self {
        let settings = SettingsState::new();
        let colors = settings.colors();
        Self { settings, colors }
    }
}

impl Default for SettingsApp {
    fn default() -> Self {
        Self::new()
    }
}

/// 启动设置窗口（阻塞直到窗口关闭）。
pub fn run() -> iced::Result {
    let icon = crate::Assets::get("image/icon.png")
        .and_then(|f| iced::window::icon::from_file_data(&f.data, None).ok());

    iced::application(SettingsApp::new, update, view)
        .title("Xime 设置")
        .window(iced::window::Settings {
            icon,
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: "xime-setup".to_string(),
                ..Default::default()
            },
            ..Default::default()
        })
        .theme(theme)
        .subscription(subscription)
        .run()
}

pub fn theme(state: &SettingsApp) -> Theme {
    state.colors.iced_theme()
}

/// 后台任务结果轮询订阅。
pub fn subscription(_state: &SettingsApp) -> Subscription<Message> {
    iced::time::every(std::time::Duration::from_millis(250)).map(|_| Message::BackgroundPoll)
}

pub fn update(state: &mut SettingsApp, message: Message) -> Task<Message> {
    match message {
        Message::PageSelected(i) => {
            state.settings.current_page = i;
        }
        Message::SchemaTab(i) => {
            state.settings.input_schema.current_tab = i;
        }
        Message::SelectSchema(i) => {
            state.settings.input_schema.selected_schema = i;
            state.settings.input_schema.config_loaded = false;
            state.settings.load_schema_config();
            match state.settings.save_schema() {
                Ok(_) => {
                    state.settings.deploy_message = Some("已切换当前输入方案".to_string());
                }
                Err(e) => {
                    state.settings.deploy_message = Some(format!("切换方案失败: {}", e));
                }
            }
        }
        Message::DeploySchemas => {
            state.settings.start_deploy();
        }
        Message::InstallSchema(id) => {
            state.settings.install_market_schema(&id);
        }
        Message::UninstallSchema(id) => {
            state.settings.uninstall_market_schema(&id);
        }
        Message::DownloadSchema(id) => {
            state.settings.download_market_schema(&id);
        }
        Message::MarketRetry => {
            state.settings.market_schema.loading = false;
            state.settings.market_schema.error = None;
            state.settings.market_schema.loaded = false;
            state.settings.start_load_market();
        }
        Message::FontSizeChanged(v) => {
            state.settings.appearance.font_size = v;
        }
        Message::CandidateCountChanged(v) => {
            state.settings.appearance.candidate_count = v;
        }
        Message::CornerRadiusChanged(v) => {
            state.settings.appearance.corner_radius = v;
        }
        Message::SaveAppearance => {
            match state.settings.save_appearance() {
                Ok(_) => {
                    state.settings.deploy_message = Some("外观设置已保存并重载".to_string());
                }
                Err(e) => {
                    state.settings.deploy_message = Some(format!("保存失败: {}", e));
                }
            }
        }
        #[cfg(feature = "smart-suggestion-page")]
        Message::SaveSmartSuggestion => {
            state.settings.deploy_message = Some("功能开发中".to_string());
        }
        #[cfg(feature = "clipboard-page")]
        Message::ClearClipboardHistory => {
            state.settings.deploy_message = Some("功能开发中".to_string());
        }
        #[cfg(feature = "pair-page")]
        Message::StartPairing => {
            state.settings.deploy_message = Some("功能开发中".to_string());
        }
        Message::BackgroundPoll => {
            state.settings.poll_background();
        }
    }
    Task::none()
}

pub fn view(state: &SettingsApp) -> Element<'_, Message> {
    let colors = &state.colors;
    let items = pages::sidebar_items();
    let current = state
        .settings
        .current_page
        .min(items.len().saturating_sub(1));

    let mut content = column![pages::page_content(&state.settings, current, colors)]
        .width(Length::Fill);

    if let Some(msg) = &state.settings.deploy_message {
        content = content.push(pages::status_line(msg, colors));
    }

    let content_scroll = pages::scrollable_content(content, colors);

    container(
        row![
            pages::sidebar(current, colors),
            container(content_scroll)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_| container_style(colors)),
        ]
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| container_style(colors))
    .into()
}

fn container_style(colors: &ThemeColors) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Color(colors.background)),
        text_color: Some(colors.foreground),
        ..iced::widget::container::Style::default()
    }
}
