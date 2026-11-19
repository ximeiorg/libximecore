//! 语音服务内核：模型目录 + 引擎生命周期 + 一次会话的状态机。
//!
//! 没有平台代码——设备、dtype、目录、空闲卸载时长都由 [`ServiceConfig`] 注入，
//! 所以 macOS / Windows / Linux 的守护进程共用这一份实现。
//!
//! 状态机（`status.state`）：
//!
//! ```text
//!            ┌──────────── 装载完成 ────────────┐
//!   idle ──► loading                            ├──► idle
//!     │                                          │
//!     └──── listen_start ──► listening ── listen_stop ──► transcribing ──► idle
//! ```
//!
//! 录音与识别都在后台线程：`listen_stop` 立刻返回，客户端轮询 `status` 等
//! `state` 回到 `idle` / 拿到 `text`。设置页 250ms 一次的轮询本来就长这样。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use candle_core::DType;
use xime_infer::DeviceKind;

use crate::capture::{self, CaptureHandle, CaptureTap};
use crate::engine::{self, Recognizer};
use crate::protocol::{ModelInfo, Status};
use crate::registry::{Entry, Registry};
use crate::stream::{StreamSession, StreamTuning};

/// 服务配置。平台差异全部在这里，服务内部不做 `cfg(target_os)`。
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// 模型根目录（每个子目录一个模型）。
    pub models_dir: PathBuf,
    pub device: DeviceKind,
    pub dtype: DType,
    /// 引擎空闲多久后卸载（把 1.6 GB 还给系统）。
    pub idle_unload: Duration,
    /// 后端展示名，例如 `CPU` / `Metal` / `CUDA`。
    pub provider: String,
}

impl ServiceConfig {
    pub fn new(models_dir: impl Into<PathBuf>) -> Self {
        Self {
            models_dir: models_dir.into(),
            device: DeviceKind::Cpu,
            dtype: DType::F32,
            idle_unload: Duration::from_secs(300),
            provider: "CPU".to_string(),
        }
    }
}

/// 服务。
pub struct SpeechService {
    cfg: ServiceConfig,
    registry: Registry,
    inner: Mutex<Inner>,
    engine: Arc<EngineSlot>,
}

struct Inner {
    /// 模型列表缓存（导入/删除/切换时刷新，status 只做克隆）。
    models: Vec<ModelInfo>,
    selected: String,
    selected_name: String,
    text: String,
    /// 已定稿文本：流式听写中只会追加，不会改写。
    committed: String,
    /// 未定稿预览：每次重解码整体替换。
    partial: String,
    partial_rev: u64,
    error: Option<String>,
    capture: Option<CaptureHandle>,
    /// 流式 worker（引擎不支持流式时为 None）。
    stream: Option<StreamWorker>,
    /// 流式已经消费的 16 kHz 样本数。
    stream_consumed: usize,
    listening: bool,
    transcribing: bool,
    models_rev: u64,
    last_used: Instant,
}

struct EngineSlot {
    state: Mutex<EngineState>,
    ready: Condvar,
}

/// 流式 worker 句柄。`stop` 置位后 worker 跑完最后一次窗口解码再退出。
///
/// 退出信号走 channel 而不是 `join()`：模型还在装载时 worker 会阻塞在引擎锁上，
/// 而松手必须立刻响应（`listen_stop` 最多等 [`STREAM_JOIN_TIMEOUT`]）。
struct StreamWorker {
    stop: Arc<AtomicBool>,
    done: std::sync::mpsc::Receiver<()>,
}

/// 松手后等流式 worker 收尾的上限（最后一轮窗口解码）。
const STREAM_JOIN_TIMEOUT: Duration = Duration::from_millis(1500);

struct EngineState {
    engine: Option<Box<dyn Recognizer>>,
    loading: bool,
    error: Option<String>,
}

/// 引擎是否正在装载（后台线程）；用于状态展示。
static LOAD_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

impl SpeechService {
    /// 创建服务并在后台装载上次选中的模型（没有则选第一个可用的）。
    pub fn new(cfg: ServiceConfig) -> Self {
        let registry = Registry::new(cfg.models_dir.clone());
        let entries = registry.scan();
        let selected = registry
            .selected()
            .or_else(|| entries.iter().find(|e| e.ready).map(|e| e.id.clone()))
            .unwrap_or_default();
        let selected_name = entries
            .iter()
            .find(|e| e.id == selected)
            .map(|e| e.name.clone())
            .unwrap_or_default();

        let service = Self {
            registry,
            inner: Mutex::new(Inner {
                models: to_infos(&entries, &selected),
                selected: selected.clone(),
                selected_name,
                text: String::new(),
                committed: String::new(),
                partial: String::new(),
                partial_rev: 0,
                error: None,
                capture: None,
                stream: None,
                stream_consumed: 0,
                listening: false,
                transcribing: false,
                models_rev: 0,
                last_used: Instant::now(),
            }),
            engine: Arc::new(EngineSlot {
                state: Mutex::new(EngineState {
                    engine: None,
                    loading: false,
                    error: None,
                }),
                ready: Condvar::new(),
            }),
            cfg,
        };
        if !selected.is_empty() {
            service.spawn_load();
        }
        service
    }

    /// 测试/嵌入用：直接塞一个已就绪的引擎，不碰真实模型。
    pub fn with_recognizer(cfg: ServiceConfig, recognizer: Box<dyn Recognizer>) -> Self {
        let registry = Registry::new(cfg.models_dir.clone());
        let entries = registry.scan();
        let selected = entries.first().map(|e| e.id.clone()).unwrap_or_default();
        let selected_name = entries.first().map(|e| e.name.clone()).unwrap_or_default();
        Self {
            registry,
            inner: Mutex::new(Inner {
                models: to_infos(&entries, &selected),
                selected,
                selected_name,
                text: String::new(),
                committed: String::new(),
                partial: String::new(),
                partial_rev: 0,
                error: None,
                capture: None,
                stream: None,
                stream_consumed: 0,
                listening: false,
                transcribing: false,
                models_rev: 0,
                last_used: Instant::now(),
            }),
            engine: Arc::new(EngineSlot {
                state: Mutex::new(EngineState {
                    engine: Some(recognizer),
                    loading: false,
                    error: None,
                }),
                ready: Condvar::new(),
            }),
            cfg,
        }
    }

    pub fn config(&self) -> &ServiceConfig {
        &self.cfg
    }

    /// 当前状态。
    pub fn status(&self) -> Status {
        let (
            models,
            selected,
            selected_name,
            text,
            committed,
            partial,
            partial_rev,
            error,
            listening,
            transcribing,
            rev,
            level,
        ) = {
            let inner = self.inner.lock().unwrap();
            (
                inner.models.clone(),
                inner.selected.clone(),
                inner.selected_name.clone(),
                inner.text.clone(),
                inner.committed.clone(),
                inner.partial.clone(),
                inner.partial_rev,
                inner.error.clone(),
                inner.listening,
                inner.transcribing,
                inner.models_rev,
                inner.capture.as_ref().map(|c| c.level()).unwrap_or(0),
            )
        };
        let engine_error = self.engine.error();
        let state = if listening {
            "listening"
        } else if transcribing {
            "transcribing"
        } else if self.engine.is_loading() {
            "loading"
        } else if engine_error.is_some() {
            "error"
        } else {
            "idle"
        };

        Status {
            state: state.to_string(),
            model_id: selected,
            model_name: selected_name,
            model_ready: self.engine.is_loaded(),
            provider: format!("{} · {}", self.cfg.provider, self.cfg.models_dir.display()),
            text,
            level,
            committed,
            partial,
            partial_rev,
            error: error.or(engine_error),
            models_rev: rev,
            models,
        }
    }

    /// 模型列表（与 status 里的同一份缓存）。
    pub fn models(&self) -> Vec<ModelInfo> {
        self.inner.lock().unwrap().models.clone()
    }

    /// 切换模型：写选中项、卸载旧引擎、后台装新的。
    pub fn select(&self, model_id: &str) -> Result<Status, String> {
        {
            let inner = self.inner.lock().unwrap();
            if inner.listening || inner.transcribing {
                return Err("正在录音/识别，先结束再切换模型".to_string());
            }
        }
        let entry = self
            .model_entry(model_id)
            .ok_or_else(|| format!("模型「{model_id}」不存在"))?;
        if !entry.ready {
            return Err(format!("模型「{model_id}」文件不完整，重新下载或导入"));
        }
        self.registry.set_selected(model_id)?;
        self.engine.unload();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.selected = entry.id.clone();
            inner.selected_name = entry.name.clone();
            inner.text.clear();
            inner.error = None;
            let selected = inner.selected.clone();
            inner.models = to_infos(&self.registry_entries(), &selected);
        }
        self.spawn_load();
        Ok(self.status())
    }

    /// 导入模型目录（复制进模型根目录）。
    pub fn import(&self, path: &str) -> Result<(String, Status), String> {
        let id = self.registry.import(std::path::Path::new(path))?;
        self.refresh_models();
        let status = self.status();
        Ok((id, status))
    }

    /// 删除模型目录；删的是当前模型时顺带卸载引擎。
    pub fn remove(&self, model_id: &str) -> Result<Status, String> {
        {
            let inner = self.inner.lock().unwrap();
            if inner.listening || inner.transcribing {
                return Err("正在录音/识别，先结束再删除模型".to_string());
            }
        }
        self.registry.remove(model_id)?;
        let selected = {
            let mut inner = self.inner.lock().unwrap();
            if inner.selected == model_id {
                inner.selected.clear();
                inner.selected_name.clear();
                inner.text.clear();
                self.engine.unload();
            }
            inner.selected.clone()
        };
        let entries = self.registry_entries();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.models = to_infos(&entries, &selected);
            inner.models_rev += 1;
        }
        Ok(self.status())
    }

    /// 开始录音。引擎没装好也会先录，`listen_stop` 时等装载完成再识别。
    ///
    /// 引擎支持流式时同时起一个后台 worker：录音过程中逐秒编码、滚动重解码，
    /// 定稿的文本实时写进 `status.committed`，客户端轮询即可上屏。
    pub fn listen_start(self: &Arc<Self>) -> Result<Status, String> {
        let already = {
            let mut inner = self.inner.lock().unwrap();
            if inner.listening {
                true
            } else {
                if inner.selected.is_empty() {
                    return Err("还没有可用的语音模型".to_string());
                }
                let capture = capture::start()?;
                inner.capture = Some(capture);
                inner.listening = true;
                inner.transcribing = false;
                inner.text.clear();
                inner.committed.clear();
                inner.partial.clear();
                inner.partial_rev += 1;
                inner.stream_consumed = 0;
                inner.error = None;
                false
            }
        };
        if already {
            return Ok(self.status());
        }
        if !self.engine.is_loaded() && !self.engine.is_loading() {
            self.spawn_load();
        }
        self.spawn_stream();
        Ok(self.status())
    }

    /// 结束录音并异步识别；结果写进 `status.text`。
    ///
    /// 流式已经出过字的话，直接采用流式结果（松手只补最后不到一秒的尾巴，
    /// 不再整段重跑一次，避免屏幕上已经落定的字被改写）。
    pub fn listen_stop(self: &Arc<Self>) -> Result<Status, String> {
        let (capture, stream) = {
            let mut inner = self.inner.lock().unwrap();
            if !inner.listening {
                return Err("当前没有在录音".to_string());
            }
            inner.listening = false;
            inner.transcribing = true;
            (inner.capture.take(), inner.stream.take())
        };
        let Some(capture) = capture else {
            return Err("录音句柄丢失".to_string());
        };

        // 先停流式 worker（它会做最后一次窗口解码并写回结果），再收录音线程。
        let streamed = stream.is_some();
        if let Some(worker) = stream {
            worker.stop.store(true, Ordering::Relaxed);
            // 超时就不等了：worker 持有 Arc，收尾完成后自行退出，不会泄漏状态
            // （它写的都是文本字段，此时 listening 已经置 false）。
            let _ = worker.done.recv_timeout(STREAM_JOIN_TIMEOUT);
        }

        let pcm = capture.stop();
        let streamed_text = {
            let inner = self.inner.lock().unwrap();
            inner.committed.clone()
        };

        if streamed && !streamed_text.is_empty() {
            let mut inner = self.inner.lock().unwrap();
            inner.transcribing = false;
            inner.text = inner.committed.clone();
            inner.partial.clear();
            inner.partial_rev += 1;
            inner.last_used = Instant::now();
            drop(inner);
            return Ok(self.status());
        }

        if pcm.len() < capture::TARGET_RATE as usize / 5 {
            let mut inner = self.inner.lock().unwrap();
            inner.transcribing = false;
            inner.error = Some("没有录到声音".to_string());
            drop(inner);
            return Ok(self.status());
        }

        let this = Arc::clone(self);
        thread::Builder::new()
            .name("xime-asr-transcribe".to_string())
            .spawn(move || {
                let result = this.transcribe_pcm(&pcm);
                let mut inner = this.inner.lock().unwrap();
                inner.transcribing = false;
                inner.last_used = Instant::now();
                match result {
                    Ok(text) => {
                        inner.text = text;
                        inner.error = None;
                    }
                    Err(error) => inner.error = Some(error),
                }
            })
            .map_err(|e| format!("启动识别线程失败：{e}"))?;
        Ok(self.status())
    }

    /// 起流式 worker。引擎不支持流式（或已被占用）时静默跳过，`listen_stop`
    /// 会退回整段识别。
    fn spawn_stream(self: &Arc<Self>) {
        // 已知不支持流式的引擎（如 Fun-ASR-Nano）连线程都不必起：整段识别更快，
        // 也避免录音期间反复报「不支持分块编码」。模型还在装载时先照常起线程，
        // worker 会在装载结束后自行判断并退出。
        if self.engine.streaming_capability() == Some(false) {
            return;
        }
        let tap = {
            let inner = self.inner.lock().unwrap();
            inner.capture.as_ref().map(|c| c.tap())
        };
        let Some(tap) = tap else {
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = std::sync::mpsc::channel();
        let service = Arc::clone(self);
        let flag = Arc::clone(&stop);
        let spawn = thread::Builder::new()
            .name("xime-asr-stream".to_string())
            .spawn(move || {
                service.stream_loop(tap, flag);
                let _ = done_tx.send(());
            });
        match spawn {
            Ok(_) => {
                let mut inner = self.inner.lock().unwrap();
                inner.stream = Some(StreamWorker { stop, done });
            }
            Err(error) => {
                // 线程起不来不影响录音：退回整段识别。
                eprintln!("[xime-speech] 启动流式线程失败：{error}");
            }
        }
    }

    /// 流式 worker 主循环：按整秒读增量音频 → 编码 → 滚动重解码 → 写回状态。
    fn stream_loop(self: &Arc<Self>, tap: CaptureTap, stop: Arc<AtomicBool>) {
        // 模型可能还在装载：`with` 会等到装载结束。不支持流式的引擎在这里直接退场，
        // `listen_stop` 见 committed 为空就会退回整段识别。
        let streamable = self
            .engine
            .with(|engine| Ok(engine.supports_streaming()))
            .unwrap_or(false);
        if !streamable {
            return;
        }
        let tuning = match self.cfg.device {
            DeviceKind::Cpu => StreamTuning::cpu(),
            _ => StreamTuning::gpu(),
        };
        let mut session = StreamSession::new(tuning.clone());
        let rate = tap.device_rate();
        let per_sec = rate as usize;
        eprintln!(
            "[xime-speech] 流式 worker 启动：后端 {:?}，窗口 {:.1}s，设备采样率 {rate}",
            self.cfg.device, tuning.window_secs
        );
        let mut cursor = 0usize;
        let mut raw_acc: Vec<f32> = Vec::new();
        let mut consumed16 = 0usize;
        let mut reported: Option<String> = None;

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let (chunk, new_cursor) = tap.read_since(cursor);
            cursor = new_cursor;
            raw_acc.extend_from_slice(&chunk);
            // 已取走的样本立刻从录音缓冲里裁掉：缓冲因此恒定在「最近
            // KEEP_SECS 秒」，长听写不会把它撑满（旧实现在攒满 30 秒后
            // 就不再接收新音频，表现为说一会儿突然一个字都不出）。
            tap.trim_before(cursor.saturating_sub(rate as usize * capture::KEEP_SECS as usize));

            // 只按整秒切：重采样在非整秒边界会引入相位不连续，而且音频塔本来就是
            // 1 秒一个 chunk，按整秒对齐刚好与它重合。
            let mut pcm16: Vec<f32> = Vec::new();
            while raw_acc.len() >= per_sec {
                let block: Vec<f32> = raw_acc.drain(..per_sec).collect();
                if rate == capture::TARGET_RATE {
                    pcm16.extend_from_slice(&block);
                } else {
                    match xime_infer::asr::audio::resample(&block, rate, capture::TARGET_RATE) {
                        Ok(block) => pcm16.extend(block),
                        Err(error) => {
                            let error = format!("重采样失败：{error}");
                            if reported.as_deref() != Some(error.as_str()) {
                                eprintln!("[xime-speech] {error}");
                                reported = Some(error);
                            }
                        }
                    }
                }
            }
            if pcm16.is_empty() {
                // 没有新音频也要转一圈：停顿定稿靠这里推进。
                thread::sleep(Duration::from_millis(60));
                let level = tap.level();
                let _ = self
                    .engine
                    .with(|engine| session.feed(engine, &[], level, Instant::now()))
                    .map(|progress| {
                        if progress.any() {
                            self.publish(&session, consumed16, false);
                        }
                    });
                continue;
            }
            consumed16 += pcm16.len();
            let level = tap.level();
            let fed = self
                .engine
                .with(|engine| session.feed(engine, &pcm16, level, Instant::now()));
            match fed {
                Ok(progress) => {
                    if progress.any() {
                        self.publish(&session, consumed16, false);
                    }
                }
                Err(error) => {
                    if reported.as_deref() != Some(error.as_str()) {
                        reported = Some(error.clone());
                        let mut inner = self.inner.lock().unwrap();
                        inner.error = Some(error);
                    }
                }
            }
            // 一轮解码本身就要几十到几百毫秒；这里只让出极短的间隙，
            // 真正节流在 StreamSession 里（按 min_interval 判 due）。
            thread::sleep(Duration::from_millis(20));
        }

        // 收尾：把尾巴编码进去、跑最后一轮，结果全部定稿。
        if !session.committed().is_empty() || consumed16 > 0 {
            let _ = self.engine.with(|engine| session.finish(engine));
            self.publish(&session, consumed16, true);
        }
    }

    /// 把会话文本写进状态。`final_round` 时同时落 `text`。
    fn publish(&self, session: &StreamSession, consumed16: usize, final_round: bool) {
        let committed = session.committed().to_string();
        let partial = session.partial();
        let rev = session.partial_rev();
        let mut inner = self.inner.lock().unwrap();
        inner.committed = committed.clone();
        inner.partial = partial;
        inner.partial_rev = rev;
        inner.stream_consumed = consumed16;
        // 解码失败也要露出来：只写日志的话，客户端看到的就是「一直不出字」。
        // 成功一轮就自动清空，所以偶发失败不会一直挂着。
        inner.error = session.error().map(str::to_string);
        if final_round {
            inner.text = committed;
        }
    }

    /// 直接识别一段 16 kHz PCM（测试与离线试听用）。
    pub fn transcribe_pcm(&self, pcm: &[f32]) -> Result<String, String> {
        if pcm.is_empty() {
            return Err("没有音频".to_string());
        }
        let mut text = String::new();
        self.engine.with(|engine| {
            text = engine.transcribe(pcm)?;
            Ok(())
        })?;
        Ok(text)
    }

    /// 空闲卸载 / 清理（守护进程每秒调一次）。
    pub fn maintain(&self) {
        let idle = {
            let inner = self.inner.lock().unwrap();
            !inner.listening
                && !inner.transcribing
                && inner.last_used.elapsed() > self.cfg.idle_unload
        };
        if idle && self.engine.is_loaded() {
            self.engine.unload();
        }
    }

    /// 退出前清理。
    pub fn shutdown(&self) {
        let (capture, stream) = {
            let mut inner = self.inner.lock().unwrap();
            (inner.capture.take(), inner.stream.take())
        };
        if let Some(worker) = stream {
            worker.stop.store(true, Ordering::Relaxed);
            let _ = worker.done.recv_timeout(STREAM_JOIN_TIMEOUT);
        }
        drop(capture); // drop 会断开通道，采集线程自行退出
        self.engine.unload();
    }

    fn refresh_models(&self) {
        let selected = {
            let inner = self.inner.lock().unwrap();
            inner.selected.clone()
        };
        let entries = self.registry_entries();
        let mut inner = self.inner.lock().unwrap();
        inner.models = to_infos(&entries, &selected);
        inner.models_rev += 1;
    }

    fn registry_entries(&self) -> Vec<Entry> {
        self.registry.scan()
    }

    fn model_entry(&self, id: &str) -> Option<Entry> {
        self.registry_entries().into_iter().find(|e| e.id == id)
    }

    /// 后台装载当前选中的模型。
    fn spawn_load(&self) {
        let dir = {
            let inner = self.inner.lock().unwrap();
            if inner.selected.is_empty() {
                return;
            }
            self.registry.root().join(&inner.selected)
        };
        {
            let mut state = self.engine.state.lock().unwrap();
            if state.loading {
                return;
            }
            state.loading = true;
            state.error = None;
        }
        LOAD_IN_FLIGHT.store(true, Ordering::SeqCst);

        let slot = Arc::clone(&self.engine);
        let (device, dtype) = (self.cfg.device, self.cfg.dtype);
        let spawned = thread::Builder::new()
            .name("xime-asr-load".to_string())
            .spawn(move || {
                let loaded = engine::load(&dir, device, dtype);
                let mut state = slot.state.lock().unwrap();
                match loaded {
                    Ok(engine) => {
                        state.engine = Some(engine);
                        state.error = None;
                    }
                    Err(error) => {
                        state.engine = None;
                        state.error = Some(error);
                    }
                }
                state.loading = false;
                LOAD_IN_FLIGHT.store(false, Ordering::SeqCst);
                slot.ready.notify_all();
            });
        if spawned.is_err() {
            let mut state = self.engine.state.lock().unwrap();
            state.loading = false;
            state.error = Some("无法创建模型装载线程".to_string());
            LOAD_IN_FLIGHT.store(false, Ordering::SeqCst);
        }
    }
}

impl EngineSlot {
    /// 已装载引擎是否支持流式；未装载（或正在装载）时返回 `None`，不阻塞调用方。
    fn streaming_capability(&self) -> Option<bool> {
        let state = self.state.lock().ok()?;
        state
            .engine
            .as_ref()
            .map(|engine| engine.supports_streaming())
    }

    fn is_loaded(&self) -> bool {
        self.state
            .lock()
            .map(|s| s.engine.is_some())
            .unwrap_or(false)
    }

    fn is_loading(&self) -> bool {
        self.state.lock().map(|s| s.loading).unwrap_or(false)
    }

    fn error(&self) -> Option<String> {
        self.state.lock().ok().and_then(|s| s.error.clone())
    }

    fn unload(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.engine = None;
            state.error = None;
        }
    }

    /// 在装载进行中调用会阻塞到装载结束（一次识别本来就要等模型）。
    fn with<T>(
        &self,
        f: impl FnOnce(&mut dyn Recognizer) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.state.lock().unwrap();
        while state.loading {
            state = self.ready.wait(state).unwrap();
        }
        match state.engine.as_mut() {
            Some(engine) => f(engine.as_mut()),
            None => Err(state
                .error
                .clone()
                .unwrap_or_else(|| "模型未装载".to_string())),
        }
    }
}

fn to_infos(entries: &[Entry], selected: &str) -> Vec<ModelInfo> {
    entries
        .iter()
        .map(|entry| ModelInfo {
            id: entry.id.clone(),
            name: entry.name.clone(),
            description: entry.description.clone(),
            size: entry.size_label(),
            downloaded: entry.ready,
            selected: entry.id == selected,
            recommended: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRecognizer {
        text: String,
    }

    impl Recognizer for FakeRecognizer {
        fn transcribe(&mut self, pcm: &[f32]) -> Result<String, String> {
            if pcm.is_empty() {
                return Err("没有音频".to_string());
            }
            Ok(self.text.clone())
        }
    }

    fn model_dir(root: &std::path::Path, id: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        std::fs::write(dir.join("tokenizer.json"), "{}").unwrap();
        std::fs::write(dir.join("model.safetensors"), vec![0u8; 8]).unwrap();
    }

    fn service(root: &std::path::Path) -> SpeechService {
        SpeechService::with_recognizer(
            ServiceConfig::new(root),
            Box::new(FakeRecognizer {
                text: "今天天气不错".to_string(),
            }),
        )
    }

    #[test]
    fn status_lists_models_and_selection() {
        let tmp = tempfile::tempdir().unwrap();
        model_dir(tmp.path(), "qwen3-asr-0.6b");
        model_dir(tmp.path(), "other");
        let service = service(tmp.path());

        let status = service.status();
        assert_eq!(status.models.len(), 2);
        assert!(status.model_id.is_empty() || status.model_id == "other");
        assert_eq!(status.state, "idle");
        // 假引擎已就绪，但不代表「模型目录完整」，两者分开看。
        assert!(status.model_ready);

        let selected = service.select("qwen3-asr-0.6b").unwrap();
        assert_eq!(selected.model_id, "qwen3-asr-0.6b");
        assert!(selected
            .models
            .iter()
            .any(|m| m.selected && m.id == "qwen3-asr-0.6b"));
    }

    #[test]
    fn select_rejects_unknown_and_incomplete_models() {
        let tmp = tempfile::tempdir().unwrap();
        model_dir(tmp.path(), "ok");
        let broken = tmp.path().join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("config.json"), "{}").unwrap();
        let service = service(tmp.path());

        assert!(service.select("nope").is_err());
        let error = service.select("broken").unwrap_err();
        assert!(error.contains("不完整"), "{error}");
    }

    #[test]
    fn transcribe_pcm_uses_engine() {
        let tmp = tempfile::tempdir().unwrap();
        model_dir(tmp.path(), "m");
        let service = service(tmp.path());
        let text = service.transcribe_pcm(&[0.0; 1600]).unwrap();
        assert_eq!(text, "今天天气不错");
        assert!(service.transcribe_pcm(&[]).is_err());
    }

    #[test]
    fn remove_clears_selection_and_refreshes() {
        let tmp = tempfile::tempdir().unwrap();
        model_dir(tmp.path(), "m1");
        model_dir(tmp.path(), "m2");
        let service = service(tmp.path());
        service.select("m1").unwrap();
        let status = service.remove("m1").unwrap();
        assert_eq!(status.models.len(), 1);
        assert!(status.model_id.is_empty(), "删掉当前模型后不该还指向它");
    }

    #[test]
    fn idle_unload_drops_engine() {
        let tmp = tempfile::tempdir().unwrap();
        model_dir(tmp.path(), "m");
        let mut cfg = ServiceConfig::new(tmp.path());
        cfg.idle_unload = Duration::from_millis(0);
        let service = SpeechService::with_recognizer(
            cfg,
            Box::new(FakeRecognizer {
                text: "x".to_string(),
            }),
        );
        service.maintain();
        assert!(!service.status().model_ready, "空闲超时后引擎应被卸载");
    }
}
