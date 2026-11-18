//! 麦克风采集。
//!
//! `cpal::Stream` 在 macOS 上不是 `Send`，所以采集固定跑在自己的线程里：
//! 线程持有 stream，回调把样本推进共享缓冲；`stop()` 发一个信号，线程把
//! stream 丢掉并把（重采样到 16 kHz 的）PCM 还给调用方。
//!
//! 录音在守护进程里做，不在输入法/设置进程里做：麦克风权限与「谁在录音」
//! 必须是同一个进程，否则 TCC 授权会落到错误的 bundle 上。

use std::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// 目标采样率：ASR 引擎只吃 16 kHz。
pub const TARGET_RATE: u32 = 16_000;

/// 模型单次整段识别的上限（秒）。非流式回退路径用得到。
pub const MAX_SECS: u32 = 30;

/// 缓冲的**滚动保留**长度（秒）——不是录音时长上限。
///
/// 录音可以持续任意长：消费方（流式 worker）每轮把已取走的样本裁掉，缓冲
/// 只保留最近这么一段。取 45 秒是为了同时兜住两件事：模型 30 秒的整段窗口，
/// 以及消费方偶尔落后几秒仍能从缓冲里补齐。
pub const KEEP_SECS: u32 = 45;

/// 正在进行的录音。`stop()` 之后才能拿到样本。
pub struct CaptureHandle {
    stop_tx: mpsc::Sender<()>,
    join: JoinHandle<Vec<f32>>,
    /// 实时输入电平 0-100（音频回调里更新，UI 轮询读取）。
    level: Arc<AtomicU8>,
    /// 原始样本数（设备采样率、单声道）。音频回调里更新。
    raw_len: Arc<AtomicUsize>,
    /// 原始样本本体。流式识别在录音过程中按需增量读取，不复制整段。
    raw: Arc<Mutex<Vec<f32>>>,
    /// 已被裁掉的样本数：`raw[0]` 在整段录音里的绝对下标。
    base: Arc<AtomicUsize>,
    /// 设备采样率。起流后才确定，线程写入。
    rate: Arc<AtomicU32>,
}

impl CaptureHandle {
    /// 实时输入电平 0-100。
    pub fn level(&self) -> u8 {
        self.level.load(Ordering::Relaxed)
    }

    /// 设备采样率（流式增量读取需要自己重采样到 16 kHz）。
    pub fn device_rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed)
    }

    /// 读取自 `cursor` 之后新增的原始样本（**设备采样率**、单声道）。
    ///
    /// 返回样本与新的游标。流式会话每轮调一次，只有新增部分被复制。
    pub fn read_since(&self, cursor: usize) -> (Vec<f32>, usize) {
        let base = self.base.load(Ordering::Relaxed);
        let raw = match self.raw.lock() {
            Ok(raw) => raw,
            Err(_) => return (Vec::new(), cursor),
        };
        read_range(&raw, base, cursor)
    }

    /// 结束录音，等待线程退出并取回 16 kHz 单声道 PCM。
    pub fn stop(self) -> Vec<f32> {
        let _ = self.stop_tx.send(());
        let pcm = self.join.join().unwrap_or_default();
        self.level.store(0, Ordering::Relaxed);
        pcm
    }

    /// 取一个可跨线程持有的只读视图，供流式识别在录音过程中增量取样。
    pub fn tap(&self) -> CaptureTap {
        CaptureTap {
            raw: self.raw.clone(),
            raw_len: self.raw_len.clone(),
            base: self.base.clone(),
            level: self.level.clone(),
            rate: self.rate.clone(),
        }
    }
}

/// 录音的只读视图。
///
/// `CaptureHandle::stop` 会消费句柄，但流式识别要在录音**期间**反复取新增样本，
/// 所以把共享缓冲的读路径单独抽出来：两边同时读，互不影响。
#[derive(Clone)]
pub struct CaptureTap {
    raw: Arc<Mutex<Vec<f32>>>,
    raw_len: Arc<AtomicUsize>,
    base: Arc<AtomicUsize>,
    level: Arc<AtomicU8>,
    rate: Arc<AtomicU32>,
}

impl CaptureTap {
    /// 实时输入电平 0-100。
    pub fn level(&self) -> u8 {
        self.level.load(Ordering::Relaxed)
    }

    /// 设备采样率（增量样本是设备采样率，需自行重采样到 16 kHz）。
    pub fn device_rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed)
    }

    /// 已采集的样本总数（设备采样率口径）。
    pub fn len(&self) -> usize {
        self.raw_len.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 读取自 `cursor` 之后新增的原始样本，返回样本与建议的新游标。
    pub fn read_since(&self, cursor: usize) -> (Vec<f32>, usize) {
        let base = self.base.load(Ordering::Relaxed);
        let raw = match self.raw.lock() {
            Ok(raw) => raw,
            Err(_) => return (Vec::new(), cursor),
        };
        read_range(&raw, base, cursor)
    }

    /// 裁掉绝对游标 `cursor` 之前的样本，释放缓冲。
    ///
    /// 每轮取完样本就调一次：缓冲因此只保留「还没被消费的尾巴」，长听写不会
    /// 攒出几十秒的堆积，也就不存在「攒满以后不再接收」这种断流。
    pub fn trim_before(&self, cursor: usize) {
        let base = self.base.load(Ordering::Relaxed);
        if cursor <= base {
            return;
        }
        let Ok(mut raw) = self.raw.lock() else {
            return;
        };
        let off = (cursor - base).min(raw.len());
        if off == 0 {
            return;
        }
        raw.drain(..off);
        self.base.store(base + off, Ordering::Relaxed);
    }
}

/// 从滚动缓冲里取 `cursor` 之后的样本，返回样本与新的**绝对**游标。
///
/// 缓冲头部可能已经被裁掉（`base` 之前的内容不复存在）。此时游标落在历史
/// 区域，只能从现存最旧的样本开始给：调用方会看到「跳了一小段」，而不是像
/// 旧实现那样在缓冲满后静默收不到任何新数据（表现为说久了突然一个字不出，
/// 但波形还在动）。
fn read_range(raw: &[f32], base: usize, cursor: usize) -> (Vec<f32>, usize) {
    let end = base + raw.len();
    let start = cursor.max(base);
    if start >= end {
        return (Vec::new(), end);
    }
    (raw[start - base..].to_vec(), end)
}

/// 一块 PCM 的 RMS → 0-100 输入电平。
///
/// 门限取 -50 dBFS（静音/底噪）到 -10 dBFS（正常说话满格），与 Windows 版
/// 的包络门限一致；这里只做瞬时值，平滑（快起慢落）交给 UI 逐帧做。
fn level_from_rms(rms: f32) -> u8 {
    const FLOOR: f32 = 0.003_16;
    if rms <= FLOOR {
        return 0;
    }
    let db = 20.0 * rms.log10();
    (((db + 50.0) / 40.0).clamp(0.0, 1.0) * 100.0).round() as u8
}

/// 打开默认输入设备开始录音。
pub fn start() -> Result<CaptureHandle, String> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let level = Arc::new(AtomicU8::new(0));
    // 缓冲放在线程外面，句柄要能与录音线程同时读（流式增量取样本）。
    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let raw_len = Arc::new(AtomicUsize::new(0));
    // `raw[0]` 在整段录音里的绝对下标（裁剪掉的前缀长度）。
    let base = Arc::new(AtomicUsize::new(0));
    let rate = Arc::new(AtomicU32::new(TARGET_RATE));

    let thread_level = level.clone();
    let thread_samples = samples.clone();
    let thread_len = raw_len.clone();
    let thread_base = base.clone();
    let thread_rate = rate.clone();
    let join = thread::Builder::new()
        .name("xime-mic".to_string())
        .spawn(move || {
            let result = run_stream(
                thread_samples.clone(),
                &ready_tx,
                &stop_rx,
                thread_level,
                thread_len,
                thread_base,
                thread_rate,
            );
            if let Err(error) = result {
                // 起流失败要把原因送回去；起流成功后错误只影响停止时机。
                let _ = ready_tx.send(Err(error));
            }
            let raw = thread_samples.lock().map(|s| s.clone()).unwrap_or_default();
            raw
        })
        .map_err(|e| format!("创建录音线程失败：{e}"))?;

    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(())) => Ok(CaptureHandle {
            stop_tx,
            join,
            level,
            raw_len,
            raw: samples,
            base,
            rate,
        }),
        Ok(Err(error)) => {
            let _ = join.join();
            Err(error)
        }
        Err(RecvTimeoutError::Timeout) => {
            let _ = stop_tx.send(());
            let _ = join.join();
            Err("打开麦克风超时（检查系统权限）".to_string())
        }
        Err(RecvTimeoutError::Disconnected) => Err("麦克风线程意外退出".to_string()),
    }
}

/// 采集线程主体：建流 → 通知就绪 → 等停止信号 → 重采样。
fn run_stream(
    samples: Arc<Mutex<Vec<f32>>>,
    ready: &mpsc::Sender<Result<(), String>>,
    stop_rx: &mpsc::Receiver<()>,
    level: Arc<AtomicU8>,
    raw_len: Arc<AtomicUsize>,
    base: Arc<AtomicUsize>,
    rate_out: Arc<AtomicU32>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "没有可用的麦克风".to_string())?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("读取麦克风配置失败：{e}"))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;
    let rate = config.sample_rate.0;
    rate_out.store(rate, Ordering::Relaxed);

    // 滚动保留上限按**设备采样率**算。旧代码用 `16000 × 30 × (rate/16000)`
    // 取整，44.1 kHz 设备上只有 21.7 秒、48 kHz 上 30 秒，是听写说一会儿就
    // 断流的直接原因之一。
    let cap = rate as usize * KEEP_SECS as usize;
    let push = {
        let samples = samples.clone();
        let level = level.clone();
        let raw_len = raw_len.clone();
        let base = base.clone();
        move |interleaved: &[f32]| {
            // 电平用原始交错样本算 RMS：设备采样率/声道数不影响 dBFS 读数。
            if !interleaved.is_empty() {
                let sum: f32 = interleaved.iter().map(|s| s * s).sum();
                let rms = (sum / interleaved.len() as f32).sqrt();
                level.store(level_from_rms(rms), Ordering::Relaxed);
            }
            if let Ok(mut buffer) = samples.lock() {
                buffer.extend(
                    interleaved
                        .chunks(channels.max(1))
                        .map(|frame| frame.iter().sum::<f32>() / channels.max(1) as f32),
                );
                // 满了滚掉**最旧**的一半，绝不丢新样本。
                //
                // 旧实现是「满了就 return」，于是录音 30 秒后缓冲不再更新、
                // 消费方再也读不到增量：面板波形还在动（电平在 return 之前
                // 就更新了），文字却永远停在最后一句，看起来像「还在录但不
                // 识别」。滚掉旧数据至少能让后续语音继续被识别。
                if buffer.len() > cap {
                    let drop = buffer.len() - cap;
                    buffer.drain(..drop);
                    base.fetch_add(drop, Ordering::Relaxed);
                }
                // 流式识别按「已写入多少」增量取样本；绝对下标 = 裁掉的前缀
                // + 当前缓冲长度，裁剪不会让游标错位。
                let prefix = base.load(Ordering::Relaxed);
                raw_len.store(prefix + buffer.len(), Ordering::Relaxed);
            }
        }
    };

    let err_fn = |error| {
        // 采集错误不 panic：下次 listen_start 会重新建流。
        eprintln!("[xime-speech] 麦克风错误：{error}");
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            device.build_input_stream(&config, move |data: &[f32], _| push(data), err_fn, None)
        }
        cpal::SampleFormat::I16 => {
            // i16 转成 f32 再走同一条缓冲路径。
            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    let converted: Vec<f32> =
                        data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                    push(&converted);
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    let converted: Vec<f32> = data
                        .iter()
                        .map(|s| (*s as f32 - 32768.0) / 32768.0)
                        .collect();
                    push(&converted);
                },
                err_fn,
                None,
            )
        }
        other => return Err(format!("不支持的麦克风采样格式：{other:?}")),
    }
    .map_err(|e| format!("创建录音流失败：{e}"))?;

    stream.play().map_err(|e| format!("启动录音失败：{e}"))?;
    let _ = ready.send(Ok(()));

    // 一直录音直到调用方停止（或通道断开 = 进程退出）。
    let _ = stop_rx.recv();
    drop(stream);
    level.store(0, Ordering::Relaxed);

    // 重采样到 16 kHz：模型只认这个采样率。
    //
    // 这里拿到的是缓冲里**剩下的**尾巴（流式 worker 会边录边裁掉已消费的
    // 部分）。流式已经出过字时上层直接采用流式结果，这段尾巴只用于「流式
    // 一个字都没出」的回退识别，够模型 30 秒的窗口用。
    let raw = samples.lock().map(|s| s.clone()).unwrap_or_default();
    let resampled = if rate == TARGET_RATE {
        raw
    } else {
        xime_infer::asr::audio::resample(&raw, rate, TARGET_RATE).unwrap_or(raw)
    };
    if let Ok(mut buffer) = samples.lock() {
        *buffer = resampled;
        // 录音结束后缓冲变成 16 kHz，游标口径跟着换：前缀归零，长度重设，
        // 之后任何残留的 tap 读到的都是新口径，不会拿着旧绝对下标越界。
        base.store(0, Ordering::Relaxed);
        raw_len.store(buffer.len(), Ordering::Relaxed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_maps_silence_and_speech() {
        assert_eq!(level_from_rms(0.0), 0);
        assert_eq!(level_from_rms(0.001), 0);
        assert_eq!(level_from_rms(0.316), 100);
        assert!(level_from_rms(0.05) > 40);
    }

    #[test]
    fn target_rate_matches_engine() {
        // 引擎的 AudioFrontend 固定 16 kHz；这里改了就识别不出字。
        assert_eq!(TARGET_RATE, 16_000);
        assert_eq!(MAX_SECS, 30);
        // 滚动窗口必须比模型单次窗口长，否则回退识别拿不到整段。
        assert!(KEEP_SECS > MAX_SECS);
    }

    /// 缓冲满了以后必须还能读到新样本（旧实现是 `return`，听写说久了就断流）。
    #[test]
    fn read_range_keeps_new_samples_after_trim() {
        // base = 1000，缓冲里是绝对下标 1000..1003。
        let raw = vec![0.0f32, 1.0, 2.0];
        let (chunk, cursor) = read_range(&raw, 1000, 1002);
        assert_eq!(chunk, vec![2.0]);
        assert_eq!(cursor, 1003);

        // 游标落在已被裁掉的历史区：从现存最旧的样本开始给，而不是给空。
        let (chunk, cursor) = read_range(&raw, 1000, 10);
        assert_eq!(chunk.len(), 3);
        assert_eq!(cursor, 1003);

        // 已经读到头：返回空，但游标不能倒退。
        let (chunk, cursor) = read_range(&raw, 1000, 1003);
        assert!(chunk.is_empty());
        assert_eq!(cursor, 1003);
    }

    #[test]
    fn trim_advances_base_without_breaking_cursor() {
        let tap = CaptureTap {
            raw: Arc::new(Mutex::new(vec![0.0; 10])),
            raw_len: Arc::new(AtomicUsize::new(10)),
            base: Arc::new(AtomicUsize::new(0)),
            level: Arc::new(AtomicU8::new(0)),
            rate: Arc::new(AtomicU32::new(TARGET_RATE)),
        };
        tap.trim_before(4);
        // 裁剪不改变「整段录音已采到多少」，只改变缓冲里还剩多少。
        assert_eq!(tap.len(), 10);
        let (chunk, cursor) = tap.read_since(4);
        assert_eq!(chunk.len(), 6);
        assert_eq!(cursor, 10);

        // 幂等：重复裁同一位置不会再动缓冲。
        tap.trim_before(4);
        assert_eq!(tap.read_since(4).0.len(), 6);
    }
}
