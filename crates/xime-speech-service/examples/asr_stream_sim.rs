//! 流式听写的离线模拟：把一段 WAV 按 1 秒喂给会话，看它逐秒出什么字。
//!
//! 不碰麦克风，所以能在没有录音设备（CI、远程机）上复现流式行为与延迟。
//!
//! 用法：
//! ```text
//! cargo run --release -p xime-speech-service --example asr_stream_sim -- <模型目录> <wav>
//! ```

use std::path::Path;
use std::time::Instant;

use xime_speech_service::engine::QwenRecognizer;
use xime_speech_service::stream::{StreamSession, StreamTuning, CHUNK_SAMPLES};
use xime_speech_service::DeviceKind;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: asr_stream_sim <模型目录> <wav>");
        std::process::exit(2);
    }
    let dir = Path::new(&args[1]);
    let wav = Path::new(&args[2]);

    let device = DeviceKind::platform_default();
    let mut engine = QwenRecognizer::load(dir, device, xime_speech_service::DType::F32)?;
    println!("后端 {device:?}");

    let pcm = xime_infer::asr::audio::load_wav_16k(wav, 16_000)?;
    let secs = pcm.len() / CHUNK_SAMPLES;
    println!(
        "音频 {:.2}s，按 1 秒切 {} 块\n",
        pcm.len() as f64 / 16_000.0,
        secs
    );

    // 流式参数跟真实部署一致：GPU 长窗口、CPU 短窗口。
    let tuning = match device {
        DeviceKind::Cpu => StreamTuning::cpu(),
        _ => StreamTuning::gpu(),
    };
    println!(
        "窗口 {:.0}s / 间隔 {:?} / 一致 {} 轮\n",
        tuning.window_secs, tuning.min_interval, tuning.agreement
    );

    let mut session = StreamSession::new(tuning);
    let t0 = Instant::now();
    // 虚拟时钟：真实录音里每块音频相隔 1 秒，而这里跑得比实时快得多。
    // 会话内部的节流与停顿判断都按传入的 `now` 算，所以把时间轴垫到过去、
    // 每轮 +1 秒即可复现真实节奏（否则 11 块会在几毫秒内喂完，节流全被跳过）。
    let base = t0 - std::time::Duration::from_secs(secs as u64 + 1);
    let mut slowest = 0.0f64;

    for i in 0..secs {
        let chunk = &pcm[i * CHUNK_SAMPLES..(i + 1) * CHUNK_SAMPLES];
        let now = base + std::time::Duration::from_secs(i as u64 + 1);
        // 有声块给 60 电平，模拟有人在说话（停顿定稿靠它）。
        let t = Instant::now();
        let progress = session.feed(&mut engine, chunk, 60, now)?;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        slowest = slowest.max(ms);
        if progress.any() {
            println!(
                "[{:>2}s] {:>6.0}ms  committed={:?}  partial={:?}",
                i + 1,
                ms,
                session.committed(),
                session.partial()
            );
        }
    }

    let t = Instant::now();
    let text = session.finish(&mut engine)?;
    println!(
        "\n收尾 {:.0}ms（最慢一轮 {:.0}ms，总耗时 {:.1}s）\n全文：{}",
        t.elapsed().as_secs_f64() * 1000.0,
        slowest,
        t0.elapsed().as_secs_f64(),
        text
    );
    Ok(())
}
