//! 本地语音模型（server 侧引擎）在设置端的镜像与操作回调。
//!
//! **进程与所有权**：语音识别引擎跑在 `winxime-server` 进程里（麦克风归它所有，
//! 候选栏 🎙️ 与设置页试听共用同一条会话），设置进程只能通过宿主注入的回调去
//! 询问/驱动它。于是本模块只放三样东西：镜像类型、回调槽（`set_notify_speech_*`）、
//! 轮询状态——**不引任何 IPC 依赖**（这个库是跨平台的，不绑 Windows 命名管道）。
//!
//! 页面不在前台时不去打扰 server（`set_active`），进页面/操作后 250ms 轮询一次。

#![cfg(all(feature = "voice-page", windows))]

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 操作提示在页面上的存活时间。
const MESSAGE_TTL: Duration = Duration::from_secs(5);

/// 一个可管理的语音模型（`xime-speech` 注册表的镜像）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpeechModelEntry {
    /// 模型 id（= 模型目录名）。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 一句话描述。
    pub description: String,
    /// 下载包大小（展示用）。
    pub size: String,
    /// 四件套是否已下载完整。
    pub downloaded: bool,
    /// 是否为当前选中。
    pub selected: bool,
    /// 是否为推荐模型（页面挂「推荐」标记）。
    pub recommended: bool,
}

impl SpeechModelEntry {
    /// 下拉/卡片上的一句状态。
    pub fn state_label(&self) -> &'static str {
        if self.selected {
            "当前使用"
        } else if self.downloaded {
            "已下载"
        } else {
            "未下载"
        }
    }
}

/// 语音状态的产品化分类：页面只管「选哪个颜色、写哪句话」，判断逻辑留在这里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeechStatusKind {
    /// 还没拿到第一份状态（刚进页面，正在读）。
    Connecting,
    /// 输入法服务没运行（管道不通）。
    Offline,
    /// 服务在，但当前模型没下载完。
    ModelMissing,
    /// 正在装载模型。
    Loading,
    /// 正在听（试听中）。
    Listening,
    /// 就绪。
    Ready,
}

impl SpeechStatusKind {
    /// 状态徽标文案。
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "读取中",
            Self::Offline => "服务未运行",
            Self::ModelMissing => "模型未下载",
            Self::Loading => "装载中",
            Self::Listening => "正在听写",
            Self::Ready => "已就绪",
        }
    }

    /// 状态下方的一句人话（告诉用户「现在该干什么」）。
    pub fn hint(self) -> &'static str {
        match self {
            Self::Connecting => "正在读取输入法服务的语音状态……",
            Self::Offline => "输入法服务未运行：装好输入法后它会随输入法自动启动",
            Self::ModelMissing => "下载下面的模型后即可使用；下载一次，离线可用",
            Self::Loading => "首次装载模型需要几秒，之后就一直常驻了",
            Self::Listening => "正在听你说……说完停顿一下；试听结果不上屏",
            Self::Ready => "打字时点候选栏的 🎙️ 说话，停顿时自动上屏",
        }
    }

    /// 是否需要用户关注（页面据此决定提示色）。
    pub fn is_problem(self) -> bool {
        matches!(self, Self::Offline | Self::ModelMissing)
    }
}

/// 下拉选项：模型。
///
/// `Display` 决定下拉里显示什么——**给用户看的名字 + 大小 + 状态**，
/// 不显示 `x-asr-480ms-...` 这种工程 id（id 只在下拉下面的小字里出现）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeechModelChoice {
    /// 模型 id（选中后用它与 server 通信）。
    pub id: String,
    /// 展示文本。
    pub label: String,
}

impl std::fmt::Display for SpeechModelChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// server 侧语音状态镜像。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpeechServerStatus {
    /// 引擎状态：`idle` / `loading` / `listening`。
    pub state: String,
    /// 当前选中的模型 id。
    pub model_id: String,
    /// 当前选中的模型展示名。
    pub model_name: String,
    /// 选中模型是否已下载完整。
    pub model_ready: bool,
    /// 推理后端说明（CPU / CUDA 分包）。
    pub provider: String,
    /// 实时识别文本（试听时显示）。
    pub text: String,
    /// 最近一次错误。
    pub error: Option<String>,
    /// 正在下载的模型与进度：（模型 id, 0.0~1.0）。
    pub download: Option<(String, f32)>,
    /// 模型集合变化计数。
    pub models_rev: u64,
    /// 可管理的模型列表。
    pub models: Vec<SpeechModelEntry>,
}

impl SpeechServerStatus {
    /// 是否正在听（试听中）。
    pub fn is_listening(&self) -> bool {
        self.state == "listening"
    }

    /// 是否正在装载模型。
    pub fn is_loading(&self) -> bool {
        self.state == "loading"
    }

    /// 指定模型当前的下进度（不是它在下就返回 None）。
    pub fn download_progress(&self, model_id: &str) -> Option<f32> {
        self.download
            .as_ref()
            .filter(|(id, _)| id == model_id)
            .map(|(_, progress)| *progress)
    }
}

/// 读状态回调（设置宿主注入：IPC `GetSpeechStatus`；server 没运行返回 None）。
type StatusFn = fn() -> Option<SpeechServerStatus>;
/// 带模型 id 的操作回调（下载 / 删除 / 切换）。
type ModelActionFn = fn(&str) -> Option<SpeechServerStatus>;
/// 无参操作回调（试听起停）。
type PlainActionFn = fn() -> Option<SpeechServerStatus>;

static NOTIFY_SPEECH_STATUS: OnceLock<StatusFn> = OnceLock::new();
static NOTIFY_SPEECH_DOWNLOAD: OnceLock<ModelActionFn> = OnceLock::new();
static NOTIFY_SPEECH_DELETE: OnceLock<ModelActionFn> = OnceLock::new();
static NOTIFY_SPEECH_SELECT: OnceLock<ModelActionFn> = OnceLock::new();
static NOTIFY_SPEECH_TEST_START: OnceLock<PlainActionFn> = OnceLock::new();
static NOTIFY_SPEECH_TEST_STOP: OnceLock<PlainActionFn> = OnceLock::new();

/// 设置宿主的「读语音状态」回调。
pub fn set_notify_speech_status(f: StatusFn) {
    let _ = NOTIFY_SPEECH_STATUS.set(f);
}

/// 设置宿主的「下载语音模型」回调。
pub fn set_notify_speech_download(f: ModelActionFn) {
    let _ = NOTIFY_SPEECH_DOWNLOAD.set(f);
}

/// 设置宿主的「删除语音模型」回调。
pub fn set_notify_speech_delete(f: ModelActionFn) {
    let _ = NOTIFY_SPEECH_DELETE.set(f);
}

/// 设置宿主的「切换当前语音模型」回调。
pub fn set_notify_speech_select(f: ModelActionFn) {
    let _ = NOTIFY_SPEECH_SELECT.set(f);
}

/// 设置宿主的「开始试听」回调。
pub fn set_notify_speech_test_start(f: PlainActionFn) {
    let _ = NOTIFY_SPEECH_TEST_START.set(f);
}

/// 设置宿主的「结束试听」回调。
pub fn set_notify_speech_test_stop(f: PlainActionFn) {
    let _ = NOTIFY_SPEECH_TEST_STOP.set(f);
}

fn notify_status() -> Option<SpeechServerStatus> {
    NOTIFY_SPEECH_STATUS.get().and_then(|f| f())
}

fn notify_model_action(
    slot: &OnceLock<ModelActionFn>,
    model_id: &str,
) -> Option<SpeechServerStatus> {
    slot.get().and_then(|f| f(model_id))
}

fn notify_plain(slot: &OnceLock<PlainActionFn>) -> Option<SpeechServerStatus> {
    slot.get().and_then(|f| f())
}

/// 设置页「语音转文本」区的 server 侧镜像（250ms 轮询刷新）。
#[derive(Clone, Debug, Default)]
pub struct SpeechModelState {
    /// server 侧快照（None = 还没拉到）。
    pub status: Option<SpeechServerStatus>,
    /// server 没运行 / 管道不通：页面要区分「服务未运行」和「没有模型」。
    pub offline: bool,
    /// 语音页是否在前台（不在前台就不轮询，别白占 IPC）。
    active: bool,
    /// 操作提示（成功失败都显示在这一行）。
    message: Option<String>,
    /// 提示的写入时刻（5 秒后不再显示）。
    message_at: Option<Instant>,
}

impl SpeechModelState {
    /// 进出语音页时调用：进页立刻拉一次，离开就不再轮询。
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
        if active {
            self.poll_once();
        } else {
            self.message = None;
            self.message_at = None;
        }
    }

    /// 250ms 后台轮询（只在本页可见时真的发请求）。
    pub fn poll(&mut self) {
        if !self.active {
            return;
        }
        // 试听中 / 下载中即使页面被切走也要继续刷新：状态归零要能看见。
        self.poll_once();
    }

    /// 立刻拉一次状态。
    pub fn poll_once(&mut self) {
        match notify_status() {
            Some(status) => self.apply(status),
            None => {
                self.offline = true;
                self.status = None;
            }
        }
    }

    fn apply(&mut self, status: SpeechServerStatus) {
        self.offline = false;
        // server 报的错误（会话失败 / 模型操作失败）当一次提示显示。
        if let Some(error) = status.error.clone() {
            if self.message.as_deref() != Some(error.as_str()) {
                self.set_message(error);
            }
        }
        self.status = Some(status);
    }

    /// 当前模型名（没拿到状态时给占位）。
    pub fn model_name(&self) -> &str {
        match &self.status {
            Some(status) if !status.model_name.is_empty() => &status.model_name,
            _ => "（未连接服务）",
        }
    }

    /// 模型列表（没拉到状态时为空）。
    pub fn models(&self) -> &[SpeechModelEntry] {
        match &self.status {
            Some(status) => &status.models,
            None => &[],
        }
    }

    /// 当前选中的模型条目（服务没起来时为 None）。
    pub fn selected_entry(&self) -> Option<&SpeechModelEntry> {
        let id = self.selected_id();
        if id.is_empty() {
            return None;
        }
        self.models().iter().find(|entry| entry.id == id)
    }

    /// 当前选中的模型 id。
    pub fn selected_id(&self) -> &str {
        match &self.status {
            Some(status) => &status.model_id,
            None => "",
        }
    }

    /// 下拉选项（名字 + 大小 + 状态，按注册表顺序）。
    pub fn choices(&self) -> Vec<SpeechModelChoice> {
        self.models()
            .iter()
            .map(|entry| SpeechModelChoice {
                id: entry.id.clone(),
                label: format!("{} · {} · {}", entry.name, entry.size, entry.state_label()),
            })
            .collect()
    }

    /// 与「当前选中」匹配的下拉项（没有就不选，让 placeholder 显示）。
    pub fn current_choice(&self) -> Option<SpeechModelChoice> {
        let id = self.selected_id();
        self.choices().into_iter().find(|choice| choice.id == id)
    }

    /// 产品化的状态分类。
    pub fn status_kind(&self) -> SpeechStatusKind {
        match &self.status {
            // 还没拉到第一份状态 vs 拉过但管道不通：这两种不能混，
            // 否则刚进页面会闪一下「服务未运行」。
            None if self.offline => SpeechStatusKind::Offline,
            None => SpeechStatusKind::Connecting,
            Some(status) => {
                if status.is_listening() {
                    SpeechStatusKind::Listening
                } else if status.is_loading() {
                    SpeechStatusKind::Loading
                } else if !status.model_ready {
                    SpeechStatusKind::ModelMissing
                } else {
                    SpeechStatusKind::Ready
                }
            }
        }
    }

    /// 后端标签（徽标用，短）：CUDA 分包写 GPU，CPU 分包写 CPU。
    pub fn backend_label(&self) -> &'static str {
        if self.provider().contains("CUDA") {
            "GPU 可用"
        } else {
            "CPU"
        }
    }

    /// 指定模型的下进度（不是它在下就 None）。
    pub fn download_progress(&self, model_id: &str) -> Option<f32> {
        self.status
            .as_ref()
            .and_then(|status| status.download_progress(model_id))
    }

    /// 把试听文本复制到剪贴板（与 WinRT 那块同一个 arboard 通道）。
    pub fn copy_text(&self) -> bool {
        let text = self.text();
        if text.is_empty() {
            return false;
        }
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => clipboard.set_text(text.to_string()).is_ok(),
            Err(_) => false,
        }
    }

    /// 推理后端说明。
    pub fn provider(&self) -> &str {
        match &self.status {
            Some(status) => &status.provider,
            None => "未知（服务未连接）",
        }
    }

    /// 试听中的识别文本。
    pub fn text(&self) -> &str {
        match &self.status {
            Some(status) => &status.text,
            None => "",
        }
    }

    /// 试听按钮文案。
    pub fn preview_label(&self) -> &'static str {
        if self
            .status
            .as_ref()
            .map(|s| s.is_listening())
            .unwrap_or(false)
        {
            "结束试听"
        } else {
            "试听说一句"
        }
    }

    /// 下载 / 删除按钮是否可用（下载中、试听中都不许改模型目录）。
    pub fn busy(&self) -> bool {
        match &self.status {
            Some(status) => {
                status.download.is_some() || status.is_listening() || status.is_loading()
            }
            None => true,
        }
    }

    /// 操作提示（5 秒内有效）。
    pub fn message(&self) -> Option<&str> {
        let fresh = self
            .message_at
            .map(|at| at.elapsed() < MESSAGE_TTL)
            .unwrap_or(false);
        if fresh {
            self.message.as_deref()
        } else {
            None
        }
    }

    fn set_message(&mut self, text: String) {
        self.message = Some(text);
        self.message_at = Some(Instant::now());
    }

    /// 下载模型（server 侧异步下载，进度看轮询）。
    pub fn download(&mut self, model_id: &str) {
        match notify_model_action(&NOTIFY_SPEECH_DOWNLOAD, model_id) {
            Some(status) => {
                // 先看错误再 apply（status 会被移进镜像）。
                let failed = status.error.is_some();
                self.apply(status);
                if !failed {
                    self.set_message(format!("已开始下载「{}」，进度见下方", model_id));
                }
            }
            None => self.set_message("输入法服务未运行，无法下载".to_string()),
        }
    }

    /// 删除模型目录。
    pub fn delete(&mut self, model_id: &str) {
        match notify_model_action(&NOTIFY_SPEECH_DELETE, model_id) {
            Some(status) => {
                let failed = status.error.is_some();
                self.apply(status);
                if !failed {
                    self.set_message(format!("已删除「{model_id}」"));
                }
            }
            None => self.set_message("输入法服务未运行，无法删除".to_string()),
        }
    }

    /// 切换当前使用的模型。
    pub fn select(&mut self, model_id: &str) {
        match notify_model_action(&NOTIFY_SPEECH_SELECT, model_id) {
            Some(status) => {
                let failed = status.error.is_some();
                self.apply(status);
                if !failed {
                    self.set_message(format!("已切换到「{model_id}」"));
                }
            }
            None => self.set_message("输入法服务未运行，无法切换".to_string()),
        }
    }

    /// 试听起停（试听只在设置页显示，不上屏）。
    pub fn toggle_preview(&mut self) {
        let listening = self
            .status
            .as_ref()
            .map(|status| status.is_listening())
            .unwrap_or(false);
        let result = if listening {
            notify_plain(&NOTIFY_SPEECH_TEST_STOP)
        } else {
            notify_plain(&NOTIFY_SPEECH_TEST_START)
        };
        match result {
            Some(status) => self.apply(status),
            None => self.set_message("输入法服务未运行，无法试听".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个「服务在线、模型齐全」的快照，供各用例改字段。
    fn status(state: &str, model_ready: bool) -> SpeechServerStatus {
        SpeechServerStatus {
            state: state.to_string(),
            model_id: "zipformer-zh-int8".to_string(),
            model_name: "纯中文（不带标点）".to_string(),
            model_ready,
            provider: "CPU 推理（CPU 分包）· sherpa-onnx 1.13.8".to_string(),
            text: String::new(),
            error: None,
            download: None,
            models_rev: 1,
            models: vec![
                SpeechModelEntry {
                    id: "x-asr-480ms-zh-en-punct-int8".to_string(),
                    name: "中英混输（自动标点）".to_string(),
                    description: "说中文、说英文都行".to_string(),
                    size: "133.90MB".to_string(),
                    downloaded: false,
                    selected: false,
                    recommended: true,
                },
                SpeechModelEntry {
                    id: "zipformer-zh-int8".to_string(),
                    name: "纯中文（不带标点）".to_string(),
                    description: "只识别中文".to_string(),
                    size: "132.63MB".to_string(),
                    downloaded: true,
                    selected: true,
                    recommended: false,
                },
            ],
        }
    }

    #[test]
    fn status_kind_covers_every_product_state() {
        let mut state = SpeechModelState::default();
        // 刚进页面还没拉到状态：不能说「服务未运行」（会闪一下假故障）。
        assert_eq!(state.status_kind(), SpeechStatusKind::Connecting);
        state.offline = true;
        assert_eq!(state.status_kind(), SpeechStatusKind::Offline);

        state.status = Some(status("idle", true));
        assert_eq!(state.status_kind(), SpeechStatusKind::Ready);

        state.status = Some(status("idle", false));
        assert_eq!(state.status_kind(), SpeechStatusKind::ModelMissing);

        state.status = Some(status("loading", true));
        assert_eq!(state.status_kind(), SpeechStatusKind::Loading);

        // 听写中的优先级最高：正在听时不该显示「就绪」。
        state.status = Some(status("listening", false));
        assert_eq!(state.status_kind(), SpeechStatusKind::Listening);
    }

    #[test]
    fn every_status_kind_has_label_and_hint() {
        for kind in [
            SpeechStatusKind::Connecting,
            SpeechStatusKind::Offline,
            SpeechStatusKind::ModelMissing,
            SpeechStatusKind::Loading,
            SpeechStatusKind::Listening,
            SpeechStatusKind::Ready,
        ] {
            assert!(!kind.label().is_empty(), "{kind:?} 缺徽标文案");
            // 提示语是给用户「现在该干什么」的，不能只写状态名。
            assert!(kind.hint().chars().count() >= 8, "{kind:?} 的提示太短");
        }
        assert!(SpeechStatusKind::Offline.is_problem());
        assert!(SpeechStatusKind::ModelMissing.is_problem());
        assert!(!SpeechStatusKind::Ready.is_problem());
        assert!(!SpeechStatusKind::Listening.is_problem());
    }

    #[test]
    fn choices_carry_name_size_and_state_not_raw_id() {
        let mut state = SpeechModelState::default();
        state.status = Some(status("idle", true));
        let choices = state.choices();
        assert_eq!(choices.len(), 2);
        let labels: Vec<String> = choices.iter().map(|c| c.label.clone()).collect();
        assert!(
            labels[0].contains("中英混输（自动标点）"),
            "{:?}",
            labels[0]
        );
        assert!(labels[0].contains("133.90MB"), "{:?}", labels[0]);
        assert!(labels[0].contains("未下载"), "{:?}", labels[0]);
        assert!(labels[1].contains("当前使用"), "{:?}", labels[1]);
        // 下拉里不出现工程 id（那是给排错看的，放详情里）。
        assert!(labels.iter().all(|l| !l.contains("x-asr-480ms")));
    }

    #[test]
    fn selected_entry_and_backend_label_follow_status() {
        let mut state = SpeechModelState::default();
        state.status = Some(status("idle", true));
        assert_eq!(
            state.selected_entry().map(|e| e.id.as_str()),
            Some("zipformer-zh-int8")
        );
        assert_eq!(state.backend_label(), "CPU");
        assert!(state.current_choice().is_some());
        // 没状态时：不假装有选中项，也不给假后端。
        let empty = SpeechModelState::default();
        assert!(empty.selected_entry().is_none());
        assert!(empty.current_choice().is_none());
        assert_eq!(empty.selected_id(), "");
    }

    #[test]
    fn busy_blocks_model_edits_while_engine_is_working() {
        let mut state = SpeechModelState::default();
        assert!(state.busy(), "服务没起来时也不该允许改模型目录");
        state.status = Some(status("listening", true));
        assert!(state.busy());
        state.status = Some(status("idle", true));
        assert!(!state.busy());
        // 下载中同样算忙（避免同一模型两路下载）。
        if let Some(status) = state.status.as_mut() {
            status.download = Some(("zipformer-zh-int8".to_string(), 0.4));
        }
        assert!(state.busy());
        assert_eq!(state.download_progress("zipformer-zh-int8"), Some(0.4));
    }
}
