use crate::pages;
use crate::state::{Message, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{column, container, row, text};
use iced::{Background, Element, Length, Subscription, Task, Theme};

/// 设置应用根状态（iced Application 的 State）。
pub struct SettingsApp {
    pub settings: SettingsState,
    /// 当前主题颜色（由设置派生，视图与主题共用）。
    pub colors: ThemeColors,
}

/// 冷启动计时锚点：`run()` 里在 iced 初始化（图形后端探测 + 窗口创建）之前设置。
static BOOT_AT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// 首帧是否已记录（`view` 只拿到 `&SettingsApp`，用原子量只记一次）。
static FIRST_FRAME_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 单帧 / 单次后台轮询超过这个时长就记一条 warn。`BackgroundPoll` 是 250ms 一跳，
/// 单次超标就意味着 UI 线程被占住（打开设置慢、界面发涩都从这里看）。
const SLOW_TICK_WARN: std::time::Duration = std::time::Duration::from_millis(50);

impl SettingsApp {
    pub fn new() -> Self {
        let started = std::time::Instant::now();
        let settings = SettingsState::new();
        let colors = settings.colors();
        // 冷启动分段计时①：这段在窗口出现之前跑（librime 初始化、方案/配置/
        // 剪贴板/同步状态加载），是「打开设置慢」的第一嫌疑。
        tracing::info!(
            "SettingsState::new() 完成 +{}ms",
            started.elapsed().as_millis()
        );
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
    // 冷启动计时起点：iced 的图形后端探测（wgpu 建 Vulkan/DX12/GL 实例、
    // 枚举适配器）与窗口/字体初始化都在 `.run()` 内部，实测这是冷启动的
    // 大头（约 1 秒量级），与业务数据加载无关。
    BOOT_AT.get_or_init(std::time::Instant::now);
    tracing::info!("run(): iced 初始化开始（图形后端 + 窗口 + 字体）");
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
            // 本地模型状态只在本页可见时轮询（进页立刻拉一次，离开就停）。
            #[cfg(all(feature = "voice-page", windows))]
            {
                let on_voice_page = crate::pages::voice_page_index() == Some(i);
                state.settings.speech_server.set_active(on_voice_page);
            }
        }
        Message::SchemaTab(i) => {
            state.settings.input_schema.current_tab = i;
            // 进「方案词表」tab 时为当前选中方案读一次词表。
            #[cfg(windows)]
            if i == 2 {
                state.settings.schema_dict_ensure();
            }
        }
        Message::SelectSchema(i) => {
            state.settings.input_schema.selected_schema = i;
            state.settings.input_schema.config_loaded = false;
            state.settings.load_schema_config();
            // 停在「方案词表」tab 时换方案 → 立即为新方案读词表。
            #[cfg(windows)]
            if state.settings.input_schema.current_tab == 2 {
                state.settings.schema_dict_ensure();
            }
            // save_schema 返回要展示的提示语：已部署→「已切换」；没产物→后台部署中。
            match state.settings.save_schema() {
                Ok(msg) if !msg.is_empty() => state.settings.show_message(msg),
                Ok(_) => {}
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
        Message::ConfirmSchemaInstall => {
            state.settings.confirm_schema_install();
        }
        Message::CancelSchemaInstall => {
            state.settings.cancel_schema_install();
        }
        Message::RestoreBuiltinSchema => {
            state.settings.restore_builtin_schema();
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
        #[cfg(all(feature = "voice-page", windows))]
        Message::SpeechModelDownload(id) => {
            state.settings.speech_server.download(&id);
        }
        #[cfg(all(feature = "voice-page", windows))]
        Message::SpeechModelDelete(id) => {
            state.settings.speech_server.delete(&id);
        }
        #[cfg(all(feature = "voice-page", windows))]
        Message::SpeechModelSelect(id) => {
            state.settings.speech_server.select(&id);
        }
        #[cfg(all(feature = "voice-page", windows))]
        Message::SpeechPreviewToggle => {
            state.settings.speech_server.toggle_preview();
        }
        #[cfg(all(feature = "voice-page", windows))]
        Message::SpeechModelCopy => {
            if state.settings.speech_server.copy_text() {
                state.settings.show_message("识别文本已复制".to_string());
            } else {
                state.settings.show_message("没有可复制的文本".to_string());
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
        #[cfg(windows)]
        Message::DictTab(i) => {
            state.settings.dict_manage.tab = i.min(1);
            // 进入快捷短语 Tab：首次自动为当前方案载入短语表。
            if i == 1 && state.settings.dict_manage.phrase.is_none() {
                if let Some((id, name)) = state.settings.selected_schema_info() {
                    state.settings.dict_manage.phrase_open(id, name);
                }
            }
        }
        #[cfg(windows)]
        Message::DictSelect(dict) => {
            state.settings.dict_manage.browse_select(dict);
        }
        #[cfg(windows)]
        Message::DictQueryChanged(query) => {
            state.settings.dict_manage.browse_set_query(query);
        }
        #[cfg(windows)]
        Message::DictEntriesPage(page) => {
            state.settings.dict_manage.browse_page(page);
        }
        #[cfg(windows)]
        Message::DictEntryAddOpen => {
            state.settings.dict_manage.browse_add_open();
        }
        #[cfg(windows)]
        Message::DictEntryAddCancel => {
            state.settings.dict_manage.browse_add_cancel();
        }
        #[cfg(windows)]
        Message::DictEntryAddWordChanged(value) => {
            state.settings.dict_manage.browse_add_word(value);
        }
        #[cfg(windows)]
        Message::DictEntryAddCodeChanged(value) => {
            state.settings.dict_manage.browse_add_code(value);
        }
        #[cfg(windows)]
        Message::DictEntryAddCommitsChanged(value) => {
            state.settings.dict_manage.browse_add_commits(value);
        }
        #[cfg(windows)]
        Message::DictEntryAddSubmit => {
            state.settings.dict_manage.browse_add_submit();
        }
        #[cfg(windows)]
        Message::DictEntryDeleteRequest(word, code) => {
            state.settings.dict_manage.browse_delete_request(word, code);
        }
        #[cfg(windows)]
        Message::DictEntryDeleteCancel => {
            state.settings.dict_manage.browse_delete_cancel();
        }
        #[cfg(windows)]
        Message::DictEntryDeleteConfirm(word, code) => {
            state.settings.dict_manage.browse_delete_confirm(word, code);
        }
        #[cfg(windows)]
        Message::DictPhraseSchemaChanged(schema_id) => {
            // 从输入方案页的方案列表里找显示名；找不到就用 id 兜底。
            let schema_name = state
                .settings
                .input_schema
                .available_schemas
                .iter()
                .find(|info| info.schema_id == schema_id)
                .map(|info| info.name.clone())
                .unwrap_or_else(|| schema_id.clone());
            state.settings.dict_manage.phrase_schema_changed(schema_id, schema_name);
        }
        #[cfg(windows)]
        Message::DictPhraseAddOpen => {
            state.settings.dict_manage.phrase_add_open();
        }
        #[cfg(windows)]
        Message::DictPhraseEdit(index) => {
            state.settings.dict_manage.phrase_edit(index);
        }
        #[cfg(windows)]
        Message::DictPhraseDialogCancel => {
            state.settings.dict_manage.phrase_dialog_cancel();
        }
        #[cfg(windows)]
        Message::DictPhraseDialogWordChanged(value) => {
            state.settings.dict_manage.phrase_dialog_word(value);
        }
        #[cfg(windows)]
        Message::DictPhraseDialogCodeChanged(value) => {
            state.settings.dict_manage.phrase_dialog_code(value);
        }
        #[cfg(windows)]
        Message::DictPhraseDialogWeightChanged(value) => {
            state.settings.dict_manage.phrase_dialog_weight(value);
        }
        #[cfg(windows)]
        Message::DictPhraseDialogSubmit => {
            state.settings.dict_manage.phrase_dialog_submit();
        }
        #[cfg(windows)]
        Message::DictPhraseDeleteRequest(index) => {
            state.settings.dict_manage.phrase_delete_request(index);
        }
        #[cfg(windows)]
        Message::DictPhraseDeleteCancel => {
            state.settings.dict_manage.phrase_delete_cancel();
        }
        #[cfg(windows)]
        Message::DictPhraseDeleteConfirm(index) => {
            state.settings.dict_manage.phrase_delete_confirm(index);
        }
        #[cfg(windows)]
        Message::SchemaDictQueryChanged(query) => {
            state.settings.schema_dict_set_query(query);
        }
        #[cfg(windows)]
        Message::SchemaDictRefresh => {
            state.settings.schema_dict_refresh();
        }
        #[cfg(windows)]
        Message::SchemaDictPage(page) => {
            state.settings.schema_dict_page(page);
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
            let started = std::time::Instant::now();
            state.settings.poll_background();
            let spent = started.elapsed();
            if spent > SLOW_TICK_WARN {
                tracing::warn!(
                    "后台轮询偏慢：{}ms（250ms 一跳，超标即 UI 卡顿）",
                    spent.as_millis()
                );
            }
        }
    }
    Task::none()
}

pub fn view(state: &SettingsApp) -> Element<'_, Message> {
    let started = std::time::Instant::now();
    let element = view_inner(state);
    let built = started.elapsed();

    use std::sync::atomic::Ordering;
    if !FIRST_FRAME_LOGGED.swap(true, Ordering::Relaxed) {
        // 冷启动分段计时②：窗口从 `.run()` 走到能画出第一帧的全部耗时
        // （图形后端 + 窗口 + 字体/图集），减去①就是非业务开销。
        let since_run = BOOT_AT.get().map_or(0, |t| t.elapsed().as_millis());
        tracing::info!("首帧构建完成：run() 之后 +{since_run}ms（本帧 view {built:?}）");
    } else if built > SLOW_TICK_WARN {
        tracing::warn!(
            "view 构建偏慢：{}ms（每帧做磁盘扫描 / 阻塞调用会走到这里）",
            built.as_millis()
        );
    }
    element
}

fn view_inner(state: &SettingsApp) -> Element<'_, Message> {
    let colors = &state.colors;
    let items = pages::sidebar_items();
    let current = state
        .settings
        .current_page
        .min(items.len().saturating_sub(1));

    let content =
        column![pages::page_content(&state.settings, current, colors)].width(Length::Fill);

    let content_scroll = pages::scrollable_content(content, colors);

    // 全局消息条（show_message 写入，5 秒自动过期）。
    let mut main_col = column![].width(Length::Fill).height(Length::Fill);
    if let Some(msg) = state.settings.ui_message() {
        let colors_copy = *colors;
        main_col = main_col.push(
            container(text(msg.to_string()).size(13).color(colors_copy.foreground))
                .width(Length::Fill)
                .padding([6, 16])
                .style(move |_| iced::widget::container::Style {
                    background: Some(Background::Color(iced::Color {
                        a: 0.10,
                        ..colors_copy.primary
                    })),
                    ..iced::widget::container::Style::default()
                }),
        );
    }
    main_col = main_col.push(
        container(content_scroll)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container_style(colors)),
    );

    let page: Element<'_, Message> = container(
        row![pages::sidebar(current, colors), main_col]
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| container_style(colors))
    .into();

    // 安装冲突确认弹窗（输入方案页 / 扩展商店安装入口共用；未确认前不动文件）。
    crate::components::widgets::modal_dialog(
        page,
        pages::input_schema::conflict_dialog(&state.settings, colors),
        colors,
    )
}

fn container_style(colors: &ThemeColors) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Color(colors.background)),
        text_color: Some(colors.foreground),
        ..iced::widget::container::Style::default()
    }
}
