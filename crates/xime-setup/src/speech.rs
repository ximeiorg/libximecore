//! 语音转文本（v1：Windows 自带 WinRT 听写引擎）。
//!
//! 架构对齐 Android xime speech 模块：`RecognitionState` 状态机 +
//! 后台 worker 独占引擎 + 共享结果槽（Android 是回调，Rust 侧为
//! UI 250ms 轮询的 `SpeechSink`）。Android v1 用本地 zipformer 流式
//! 模型（sherpa-onnx）+ 在线插件后端；Windows v1 先接系统 SpeechRecognizer
//! （零依赖、麦克风采集由系统托管），后端抽象保留，本地模型后端可直接替换。

#![cfg(all(feature = "voice-page", windows))]

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use windows::Foundation::TypedEventHandler;
use windows::Media::SpeechRecognition::{
    SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
    SpeechRecognitionResultStatus, SpeechRecognitionScenario, SpeechRecognitionTopicConstraint,
    SpeechRecognizer,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows_core::HSTRING;
use windows_future::AsyncStatus;

/// 识别状态（对齐 Android `RecognitionState`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RecognitionState {
    #[default]
    Idle,
    Listening,
    Processing,
    Error,
}

/// 共享结果槽：worker 线程写、UI 线程轮询读（对齐 Android 回调的形态）。
#[derive(Clone, Debug, Default)]
pub struct SpeechSink {
    pub state: RecognitionState,
    /// 累计识别文本（逐短语追加）。
    pub text: String,
    /// 最近一次错误（进入 Error 态时设置）。
    pub error: Option<String>,
}

enum VoiceCommand {
    Start,
    Stop,
    Shutdown,
}

/// 语音 worker 句柄：UI 线程发命令、读结果槽；引擎全部归 worker 线程所有。
#[derive(Clone)]
pub struct VoiceHandle {
    tx: Sender<VoiceCommand>,
    pub sink: Arc<Mutex<SpeechSink>>,
}

impl VoiceHandle {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let sink = Arc::new(Mutex::new(SpeechSink::default()));
        let sink_for_worker = sink.clone();
        let _ = std::thread::Builder::new()
            .name("voice-worker".into())
            .spawn(move || run_worker(rx, sink_for_worker));
        Self { tx, sink }
    }

    pub fn start(&self) {
        let _ = self.tx.send(VoiceCommand::Start);
    }

    pub fn stop(&self) {
        let _ = self.tx.send(VoiceCommand::Stop);
    }
}

impl Drop for VoiceHandle {
    fn drop(&mut self) {
        // 设置程序退出：停会话并结束 worker（引擎随线程释放）。
        let _ = self.tx.send(VoiceCommand::Shutdown);
    }
}

fn run_worker(rx: Receiver<VoiceCommand>, sink: Arc<Mutex<SpeechSink>>) {
    // WinRT 调用需要 COM 单元；MTA 免消息泵（事件经线程池回调）。
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    // 识别器跨会话复用（约束编译一次），会话对象随 start/stop 起停。
    let mut recognizer: Option<SpeechRecognizer> = None;
    let mut session: Option<ActiveSession> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            VoiceCommand::Start => {
                session = start_session(&sink, &mut recognizer);
            }
            VoiceCommand::Stop => {
                stop_session(&sink, &mut session);
            }
            VoiceCommand::Shutdown => {
                stop_session(&sink, &mut session);
                break;
            }
        }
    }
}

struct ActiveSession {
    session: SpeechContinuousRecognitionSession,
    /// ResultGenerated 事件 token（保活到会话结束）。
    _token: i64,
}

fn start_session(
    sink: &Arc<Mutex<SpeechSink>>,
    recognizer: &mut Option<SpeechRecognizer>,
) -> Option<ActiveSession> {
    {
        let mut s = lock(sink);
        s.state = RecognitionState::Processing;
        s.error = None;
    }
    let outcome = start_session_inner(sink, recognizer);
    match outcome {
        Ok(session) => {
            lock(sink).state = RecognitionState::Listening;
            Some(session)
        }
        Err(e) => {
            let mut s = lock(sink);
            s.state = RecognitionState::Error;
            s.error = Some(e);
            None
        }
    }
}

fn start_session_inner(
    sink: &Arc<Mutex<SpeechSink>>,
    recognizer: &mut Option<SpeechRecognizer>,
) -> Result<ActiveSession, String> {
    let rec = match recognizer {
        Some(r) => r.clone(),
        None => {
            // 系统默认语音语言；听写主题约束启用自由口述。
            let r = SpeechRecognizer::new().map_err(hr_msg)?;
            let topic = SpeechRecognitionTopicConstraint::Create(
                SpeechRecognitionScenario::Dictation,
                &HSTRING::from("dictation"),
            )
            .map_err(hr_msg)?;
            r.Constraints()
                .map_err(hr_msg)?
                .Append(&topic)
                .map_err(hr_msg)?;
            let compile = r.CompileConstraintsAsync().map_err(hr_msg)?;
            let compiled = wait_operation(&compile)?;
            if compiled.Status().map_err(hr_msg)? != SpeechRecognitionResultStatus::Success {
                return Err(format!(
                    "语音引擎初始化失败（status = {:?}）；请检查系统语音识别与麦克风权限",
                    compiled.Status()
                ));
            }
            *recognizer = Some(r.clone());
            r
        }
    };
    let session = rec.ContinuousRecognitionSession().map_err(hr_msg)?;
    let sink_for_event = sink.clone();
    let token = session
        .ResultGenerated(&TypedEventHandler::<
            SpeechContinuousRecognitionSession,
            SpeechContinuousRecognitionResultGeneratedEventArgs,
        >::new(move |_, args| {
            // windows-rs 事件参数为 Ref<'_, EventArgs>；直接用引用即可。
            let Some(args) = args.as_ref() else {
                return Ok(());
            };
            if let Ok(result) = args.Result() {
                if let Ok(text) = result.Text() {
                    // 短语级结果直接追加（中文听写短语间无需分隔符）。
                    lock(&sink_for_event)
                        .text
                        .push_str(&text.to_string_lossy().to_string());
                }
            }
            Ok(())
        }))
        .map_err(hr_msg)?;
    let start = session.StartAsync().map_err(hr_msg)?;
    wait_action(&start)?;
    Ok(ActiveSession {
        session,
        _token: token,
    })
}

fn stop_session(sink: &Arc<Mutex<SpeechSink>>, session: &mut Option<ActiveSession>) {
    if let Some(active) = session.take() {
        let stopped = match active.session.StopAsync() {
            Ok(op) => wait_action(&op).is_ok(),
            Err(_) => false,
        };
        if !stopped {
            let mut s = lock(sink);
            s.state = RecognitionState::Idle;
            s.error = Some("停止听写失败，会话可能仍在后台".to_string());
            return;
        }
    }
    lock(sink).state = RecognitionState::Idle;
}

fn lock(sink: &Mutex<SpeechSink>) -> std::sync::MutexGuard<'_, SpeechSink> {
    sink.lock().unwrap_or_else(|e| e.into_inner())
}

/// HRESULT → 可读错误串。
fn hr_msg(e: windows_core::Error) -> String {
    format!("0x{:08X}", e.code().0 as u32)
}

/// 轮询等待 IAsyncAction（windows-future 0.3 无阻塞 get；操作均为毫秒~秒级）。
fn wait_action(op: &windows_future::IAsyncAction) -> Result<(), String> {
    loop {
        match op.Status().map_err(hr_msg)? {
            AsyncStatus::Completed => {
                return op.GetResults().map_err(hr_msg);
            }
            AsyncStatus::Started => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                return Err(format!(
                    "语音操作失败（0x{:08X}）",
                    op.ErrorCode()
                        .map(|e| e.0 as u32)
                        .unwrap_or(0)
                ));
            }
        }
    }
}

/// 轮询等待 IAsyncOperation<T>。
fn wait_operation<T>(
    op: &windows_future::IAsyncOperation<T>,
) -> Result<T, String>
where
    T: windows_core::RuntimeType + Clone,
{
    loop {
        match op.Status().map_err(hr_msg)? {
            AsyncStatus::Completed => {
                return op.GetResults().map_err(hr_msg);
            }
            AsyncStatus::Started => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                return Err(format!(
                    "语音操作失败（0x{:08X}）",
                    op.ErrorCode()
                        .map(|e| e.0 as u32)
                        .unwrap_or(0)
                ));
            }
        }
    }
}
