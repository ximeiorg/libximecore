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
        // 显式默认字体：通用族（Sans Serif）在 Windows 上可能被 fontdb 解析到
        // 图标字体，ASCII 渲染成符号乱码（见 components/widgets.rs UI_FONT 注释）
        .default_font(crate::components::widgets::UI_FONT)
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
            state.settings.clip_history.clear();
            state.settings.show_message("剪贴板历史已清空".to_string());
        }
        #[cfg(feature = "clipboard-page")]
        Message::ClipboardHistoryRefresh => {
            state.settings.clip_history.reload();
        }
        #[cfg(feature = "clipboard-page")]
        Message::ClipboardTab(i) => {
            state.settings.clipboard_tab = i;
        }
        #[cfg(feature = "clipboard-page")]
        Message::ClipboardHistorySelected(id) => {
            state.settings.clip_history.select(id);
        }
        #[cfg(feature = "clipboard-page")]
        Message::ClipboardHistoryRemove(id) => {
            state.settings.clip_history.remove(id);
        }
        #[cfg(feature = "clipboard-page")]
        Message::QuickSendFromHistory(id) => {
            let text = state
                .settings
                .clip_history
                .items
                .iter()
                .find(|item| item.id == id)
                .map(|item| item.text.clone());
            if let Some(text) = text {
                state.settings.quick_send.add_from_text(&text);
                state.settings.show_message("已添加到快捷发送".to_string());
            }
        }
        #[cfg(feature = "clipboard-page")]
        Message::QuickSendCodeChanged(v) => {
            state.settings.quick_send.set_draft_code(v);
        }
        #[cfg(feature = "clipboard-page")]
        Message::QuickSendContentChanged(v) => {
            state.settings.quick_send.set_draft_content(v);
        }
        #[cfg(feature = "clipboard-page")]
        Message::QuickSendOpen => {
            state.settings.quick_send.open_dialog();
        }
        #[cfg(feature = "clipboard-page")]
        Message::QuickSendCancel => {
            state.settings.quick_send.cancel_dialog();
        }
        #[cfg(feature = "clipboard-page")]
        Message::QuickSendAdd => {
            state.settings.quick_send.add_draft();
        }
        #[cfg(feature = "clipboard-page")]
        Message::QuickSendSelected(id) => {
            state.settings.quick_send.select(id);
        }
        #[cfg(feature = "clipboard-page")]
        Message::QuickSendRemove(id) => {
            state.settings.quick_send.remove(id);
        }
        #[cfg(feature = "clipboard-page")]
        Message::ClipboardHistoryPrevPage => {
            state.settings.clip_history.prev_page();
        }
        #[cfg(feature = "clipboard-page")]
        Message::ClipboardHistoryNextPage => {
            state.settings.clip_history.next_page();
        }
        #[cfg(feature = "clipboard-page")]
        Message::QuickSendPrevPage => {
            state.settings.quick_send.prev_page();
        }
        #[cfg(feature = "clipboard-page")]
        Message::QuickSendNextPage => {
            state.settings.quick_send.next_page();
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
        #[cfg(feature = "backup-page")]
        Message::BackupUrlChanged(v) => {
            state.settings.backup.url = v;
        }
        #[cfg(feature = "backup-page")]
        Message::BackupUsernameChanged(v) => {
            state.settings.backup.username = v;
        }
        #[cfg(feature = "backup-page")]
        Message::BackupPasswordChanged(v) => {
            state.settings.backup.password = v;
        }
        #[cfg(feature = "backup-page")]
        Message::BackupDirChanged(v) => {
            state.settings.backup.remote_dir = v;
        }
        #[cfg(feature = "backup-page")]
        Message::BackupProviderChanged(v) => {
            state.settings.backup.select_provider(v);
        }
        #[cfg(feature = "backup-page")]
        Message::BackupFieldChanged(key, value) => {
            state.settings.backup.set_plugin_field(key, value);
        }
        #[cfg(feature = "backup-page")]
        Message::BackupModeChanged(v) => {
            state.settings.backup.mode = v;
        }
        #[cfg(feature = "backup-page")]
        Message::BackupTest => {
            state.settings.backup.start_test();
        }
        #[cfg(feature = "backup-page")]
        Message::BackupNow => {
            state.settings.backup.start_backup();
        }
        #[cfg(feature = "backup-page")]
        Message::BackupList => {
            state.settings.backup.start_list();
        }
        #[cfg(feature = "backup-page")]
        Message::BackupRestore(path) => {
            state.settings.backup.start_restore(path);
        }
        #[cfg(feature = "backup-page")]
        Message::BackupDelete(path) => {
            state.settings.backup.start_delete(path);
        }
        #[cfg(feature = "backup-page")]
        Message::RimeSyncNow => {
            // IPC 同步等待（词典大时数秒）；完成后刷新快照目录概况。
            if crate::state::notify_sync_user_data() {
                state.settings.rime_sync.reload();
                state.settings.show_message("用户资料同步完成".to_string());
            } else {
                state
                    .settings
                    .show_message("用户资料同步失败（输入法服务未运行？）".to_string());
            }
        }
        #[cfg(all(feature = "voice-page", windows))]
        Message::SpeechToggle => {
            state.settings.speech.toggle();
        }
        #[cfg(all(feature = "voice-page", windows))]
        Message::SpeechClear => {
            state.settings.speech.clear();
        }
        #[cfg(all(feature = "voice-page", windows))]
        Message::SpeechCopy => {
            if state.settings.speech.copy_text() {
                state.settings.show_message("识别文本已复制".to_string());
            } else {
                state.settings.show_message("复制失败（无文本？）".to_string());
            }
        }
        #[cfg(windows)]
        Message::DictRefresh => {
            state.settings.dict_manage.start_refresh();
        }
        #[cfg(windows)]
        Message::DictBackup(dict) => {
            state.settings.dict_manage.start_backup(dict);
        }
        #[cfg(windows)]
        Message::DictRestore => {
            // 原生文件对话框（模态；对齐 weasel 恢复流程）。
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("用户词典快照 (*.userdb.txt)", &["userdb.txt"])
                .pick_file()
            {
                state
                    .settings
                    .dict_manage
                    .start_restore(path.display().to_string());
            }
        }
        #[cfg(windows)]
        Message::DictExport(dict) => {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("文本文件 (*.txt)", &["txt"])
                .set_file_name(format!("{dict}_export.txt"))
                .save_file()
            {
                state
                    .settings
                    .dict_manage
                    .start_export(dict, path.display().to_string());
            }
        }
        #[cfg(windows)]
        Message::DictImport(dict) => {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("文本文件 (*.txt)", &["txt"])
                .pick_file()
            {
                state
                    .settings
                    .dict_manage
                    .start_import(dict, path.display().to_string());
            }
        }
        #[cfg(feature = "clipboard-page")]
        Message::SyncPluginEnabled(v) => {
            state.settings.sync_plugin.set_enabled(v);
        }
        #[cfg(feature = "clipboard-page")]
        Message::SyncPluginSelected(v) => {
            state.settings.sync_plugin.select(v);
        }
        #[cfg(feature = "clipboard-page")]
        Message::SyncPluginFieldChanged(key, value) => {
            state.settings.sync_plugin.set_field(key, value);
        }
        #[cfg(feature = "clipboard-page")]
        Message::SyncPluginTest => {
            state.settings.sync_plugin.start_test();
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
