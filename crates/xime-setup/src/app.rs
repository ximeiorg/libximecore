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
/// 用系统默认方式打开目录（macOS `open` / Linux `xdg-open` / Windows `explorer`）。
fn open_directory(dir: &std::path::Path) {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

pub fn run() -> iced::Result {
    let icon = crate::Assets::get("image/icon.png")
        .and_then(|f| iced::window::icon::from_file_data(&f.data, None).ok());
    let meta = xime_config::app_metadata();
    let title: &'static str = Box::leak(format!("{} 设置", meta.display_name).into_boxed_str());
    iced::application(SettingsApp::new, update, view)
        .title(title)
        .window(iced::window::Settings {
            icon,
            platform_specific: iced::window::settings::PlatformSpecific {
                #[cfg(target_os = "linux")]
                application_id: format!("{}-setup", meta.config_dir_name),
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
                    state
                        .settings
                        .show_message("已切换当前输入方案".to_string());
                }
                Err(e) => {
                    state.settings.show_message(format!("切换方案失败: {}", e));
                }
            }
        }
        Message::DeploySchemas => {
            state.settings.start_deploy();
        }
        Message::OpenUserDataDir => {
            let (_, user_dir) = xime_config::get_data_dirs();
            let dir = user_dir.parent().unwrap_or(&user_dir);
            open_directory(dir);
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
        Message::DownloadModel(id) => {
            state.settings.download_market_model(&id);
        }
        Message::DeleteModel(id) => {
            state.settings.delete_market_model(&id);
        }
        Message::DownloadPlugin(id) => {
            state.settings.download_market_plugin(&id);
        }
        Message::InstallPlugin(id) => {
            state.settings.install_market_plugin(&id);
        }
        Message::UninstallPlugin(id) => {
            state.settings.uninstall_market_plugin(&id);
            state.settings.plugin_uninstall_confirm = None;
        }
        Message::ConfirmUninstallPlugin(id) => {
            state.settings.plugin_uninstall_confirm = Some(id);
        }
        Message::CancelUninstallPlugin => {
            state.settings.plugin_uninstall_confirm = None;
        }
        Message::RefreshPlugins => {
            state.settings.refresh_installed_plugins();
        }
        Message::TogglePlugin(id, enabled) => {
            state.settings.toggle_market_plugin(&id, enabled);
        }
        Message::MarketRetry => {
            state.settings.refresh_store();
        }
        Message::StoreTab(i) => {
            state.settings.market_schema.store_tab = i;
        }
        Message::StoreTagSelected(tag) => {
            let store = &mut state.settings.market_schema;
            let selected = if tag.is_empty() { None } else { Some(tag) };
            if store.store_tab == 0 {
                store.selected_tag = selected;
            } else if store.store_tab == 1 {
                state.settings.market_model.selected_tag = selected;
            } else {
                state.settings.market_plugin.selected_tag = selected;
            }
        }
        Message::SchemaVersionSelected(id, version) => {
            state
                .settings
                .market_schema
                .selected_versions
                .insert(id, version);
        }
        Message::ModelVersionSelected(id, version) => {
            state
                .settings
                .market_model
                .selected_versions
                .insert(id, version);
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
        Message::ColorSchemeLightChanged(scheme) => {
            use xime_config::ColorSchemeConfig;
            state.settings.appearance.color_scheme = match &state.settings.appearance.color_scheme {
                ColorSchemeConfig::Simple(_) => ColorSchemeConfig::Named {
                    light: scheme,
                    dark: "slate_gray".to_string(),
                },
                ColorSchemeConfig::Named { dark, .. } => ColorSchemeConfig::Named {
                    light: scheme,
                    dark: dark.clone(),
                },
            };
        }
        Message::ColorSchemeDarkChanged(scheme) => {
            use xime_config::ColorSchemeConfig;
            state.settings.appearance.color_scheme = match &state.settings.appearance.color_scheme {
                ColorSchemeConfig::Simple(_) => ColorSchemeConfig::Named {
                    light: "lavender_purple".to_string(),
                    dark: scheme,
                },
                ColorSchemeConfig::Named { light, .. } => ColorSchemeConfig::Named {
                    light: light.clone(),
                    dark: scheme,
                },
            };
        }
        Message::DarkModeChanged(mode) => {
            state.settings.appearance.dark_mode = xime_config::DarkMode::Simple(mode);
        }
        Message::SaveAppearance => match state.settings.save_color_scheme() {
            Ok(()) => match state.settings.save_appearance() {
                Ok(_) => {
                    state.colors = state.settings.colors();
                    state
                        .settings
                        .show_message("外观设置已保存并重载".to_string());
                }
                Err(e) => {
                    state.settings.show_message(format!("保存失败: {}", e));
                }
            },
            Err(e) => {
                state.settings.show_message(format!("保存配色失败: {}", e));
            }
        },
        #[cfg(feature = "smart-suggestion-page")]
        Message::SaveSmartSuggestion => {
            state.settings.show_message("功能开发中".to_string());
        }
        #[cfg(feature = "clipboard-page")]
        Message::ClearClipboardHistory => {
            state.settings.show_message("功能开发中".to_string());
        }
        #[cfg(feature = "clipboard-page")]
        Message::ServerStart => match state.settings.clipboard.spawn_server() {
            Ok(()) => {}
            Err(e) => state.settings.show_message(e),
        },
        #[cfg(feature = "clipboard-page")]
        Message::ServerStop => {
            state.settings.clipboard.stop_server();
        }
        #[cfg(feature = "clipboard-page")]
        Message::ServerRestart => {
            state.settings.clipboard.stop_server();
            match state.settings.clipboard.spawn_server() {
                Ok(()) => {}
                Err(e) => state.settings.show_message(e),
            }
        }
        #[cfg(feature = "clipboard-page")]
        Message::ServerAddrChanged(v) => {
            state.settings.clipboard.server_addr = v;
        }
        #[cfg(feature = "clipboard-page")]
        Message::ServerUsernameChanged(v) => {
            state.settings.clipboard.username = v;
        }
        #[cfg(feature = "clipboard-page")]
        Message::ServerPasswordChanged(v) => {
            state.settings.clipboard.password = v;
        }
        #[cfg(feature = "clipboard-page")]
        Message::OpenSyncDataDir => {
            let dir = std::path::PathBuf::from(&state.settings.clipboard.data_dir);
            std::fs::create_dir_all(&dir).ok();
            open_directory(&dir);
        }
        #[cfg(feature = "pair-page")]
        Message::StartPairing => {
            state.settings.show_message("功能开发中".to_string());
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

    let content =
        column![pages::page_content(&state.settings, current, colors)].width(Length::Fill);

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
