//! Qwen3-ASR 冒烟测试：装载、预热、转写一段 WAV，打印耗时。
//!
//! 用法：
//! ```text
//! cargo run --release -p xime-infer --example asr_transcribe -- <模型目录> <wav> [语种]
//! ```
//!
//! 模型目录支持两种发布格式（`-hf` 与 ModelScope 原始格式），见 `asr::config`。

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use xime_infer::asr::{AsrOptions, Qwen3Asr};

fn write_f32(path: &str, data: &[f32]) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    for v in data {
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: asr_transcribe <模型目录> <wav> [语种，如 Chinese]");
        std::process::exit(2);
    }
    let dir = Path::new(&args[1]);
    let wav = Path::new(&args[2]);
    let language = args.get(3).cloned();

    let mut opts = AsrOptions {
        language,
        ..Default::default()
    };
    // 默认后端跟着 feature 走：CUDA 包自动探测 GPU，macOS/CPU 包用 CPU。
    // 这里只处理显式覆盖（Metal 建议 f16：显存减半、解码更快，但要单实例运行）。
    match std::env::var("XIME_ASR_DEVICE").as_deref() {
        Ok("cpu") | Err(_) => {}
        #[cfg(all(target_os = "macos", feature = "metal"))]
        Ok("metal") => {
            opts.device = xime_infer::DeviceKind::Metal;
            opts.dtype = candle_core::DType::F16;
        }
        #[cfg(feature = "cuda")]
        Ok("cuda") => opts.device = xime_infer::DeviceKind::Cuda,
        Ok(other) => {
            eprintln!("未知后端：{other}（可用：cpu、metal、cuda，按构建 feature）");
            std::process::exit(2);
        }
    }
    if let Ok(dt) = std::env::var("XIME_ASR_DTYPE") {
        opts.dtype = match dt.as_str() {
            "f16" => candle_core::DType::F16,
            "bf16" => candle_core::DType::BF16,
            _ => candle_core::DType::F32,
        };
    }
    println!(
        "device = {}, dtype = {:?}",
        opts.device.as_str(),
        opts.dtype
    );

    let t0 = Instant::now();
    let mut asr = Qwen3Asr::from_dir_with(dir, opts)?;
    println!("装载模型: {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);

    let t = Instant::now();
    asr.warmup()?;
    println!("预热: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);

    if let Ok(dump) = std::env::var("XIME_ASR_DUMP") {
        let pcm = xime_infer::asr::audio::load_wav_16k(wav, 16_000)?;
        let mel = asr.mel(&pcm)?;
        let feats = asr.encode(&pcm)?;
        std::fs::create_dir_all(&dump)?;
        write_f32(&format!("{dump}/mel_f32.bin"), &mel.data)?;
        write_f32(
            &format!("{dump}/feats_f32.bin"),
            &feats.flatten_all()?.to_vec1::<f32>()?,
        )?;
        std::fs::write(
            format!("{dump}/feats_shape.txt"),
            format!("{} {}", feats.dims()[0], feats.dims()[1]),
        )?;
        println!(
            "dump → {dump}（mel {}×{}，feats {:?}）",
            mel.frames,
            mel.mel_bins,
            feats.dims()
        );
    }

    for run in 0..3 {
        let out = asr.transcribe_wav(wav)?;
        println!(
            "run {run}: {:.0} ms（编码 {:.0} + 解码 {:.0}），音频 {:.2} s，\
             {} 个音频 token，{} 个文本 token，语种 {:?}，RT {:.2}x",
            out.encode_ms + out.decode_ms,
            out.encode_ms,
            out.decode_ms,
            out.audio_ms / 1000.0,
            out.audio_tokens,
            out.text_tokens,
            out.language,
            (out.encode_ms + out.decode_ms) / out.audio_ms,
        );
        println!("  文本: {}", out.text);
        println!("  token ids: {:?}", out.token_ids);
    }

    Ok(())
}
