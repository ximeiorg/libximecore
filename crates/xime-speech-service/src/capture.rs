//! 麦克风采集。
//!
//! `cpal::Stream` 在 macOS 上不是 `Send`，所以采集固定跑在自己的线程里：
//! 线程持有 stream，回调把样本推进共享缓冲；`stop()` 发一个信号，线程把
//! stream 丢掉并把（重采样到 16 kHz 的）PCM 还给调用方。
//!
//! 录音在守护进程里做，不在输入法/设置进程里做：麦克风权限与「谁在录音」
//! 必须是同一个进程，否则 TCC 授权会落到错误的 bundle 上。

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// 目标采样率：ASR 引擎只吃 16 kHz。
pub const TARGET_RATE: u32 = 16_000;

/// 单次录音上限（秒）。模型单次窗口 30 秒，超出部分直接丢弃。
pub const MAX_SECS: u32 = 30;

/// 正在进行的录音。`stop()` 之后才能拿到样本。
pub struct CaptureHandle {
    stop_tx: mpsc::Sender<()>,
    join: JoinHandle<Vec<f32>>,
    /// 实时输入电平 0-100（音频回调里更新，UI 轮询读取）。
    level: Arc<AtomicU8>,
}

impl CaptureHandle {
    /// 实时输入电平 0-100。
    pub fn level(&self) -> u8 {
        self.level.load(Ordering::Relaxed)
    }

    /// 结束录音，等待线程退出并取回 16 kHz 单声道 PCM。
    pub fn stop(self) -> Vec<f32> {
        let _ = self.stop_tx.send(());
        let pcm = self.join.join().unwrap_or_default();
        self.level.store(0, Ordering::Relaxed);
        pcm
    }
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

    let thread_level = level.clone();
    let join = thread::Builder::new()
        .name("xime-mic".to_string())
        .spawn(move || {
            let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
            let result = run_stream(samples.clone(), &ready_tx, &stop_rx, thread_level);
            if let Err(error) = result {
                // 起流失败要把原因送回去；起流成功后错误只影响停止时机。
                let _ = ready_tx.send(Err(error));
            }
            let raw = samples.lock().map(|s| s.clone()).unwrap_or_default();
            raw
        })
        .map_err(|e| format!("创建录音线程失败：{e}"))?;

    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(())) => Ok(CaptureHandle {
            stop_tx,
            join,
            level,
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

    let cap = TARGET_RATE * MAX_SECS;
    let push = {
        let samples = samples.clone();
        let level = level.clone();
        move |interleaved: &[f32]| {
            // 电平用原始交错样本算 RMS：设备采样率/声道数不影响 dBFS 读数。
            if !interleaved.is_empty() {
                let sum: f32 = interleaved.iter().map(|s| s * s).sum();
                let rms = (sum / interleaved.len() as f32).sqrt();
                level.store(level_from_rms(rms), Ordering::Relaxed);
            }
            if let Ok(mut buffer) = samples.lock() {
                let cap = (cap as usize) * (rate as usize / TARGET_RATE as usize).max(1);
                if buffer.len() >= cap {
                    return;
                }
                buffer.extend(
                    interleaved
                        .chunks(channels.max(1))
                        .map(|frame| frame.iter().sum::<f32>() / channels.max(1) as f32),
                );
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
            let push = push;
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
            let push = push;
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
    let raw = samples.lock().map(|s| s.clone()).unwrap_or_default();
    let resampled = if rate == TARGET_RATE {
        raw
    } else {
        xime_infer::asr::audio::resample(&raw, rate, TARGET_RATE).unwrap_or(raw)
    };
    if let Ok(mut buffer) = samples.lock() {
        *buffer = resampled;
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
    }
}
