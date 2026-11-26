//! Whisper 冒烟测试：装载、预热、转写一段 WAV，打印耗时与文本。
//!
//! 用法：
//! ```text
//! cargo run --release -p xime-infer --example whisper_transcribe -- <模型目录> <wav> [语种]
//! ```
//!
//! 模型目录是 transformers 布局的 whisper 快照（`config.json` +
//! `generation_config.json` + `tokenizer.json` + `model.safetensors`），
//! 例如 ModelScope 的 `openai-mirror/whisper-small`。
//!
//! 对拍：设 `XIME_ASR_DUMP=/tmp/whisper_dump` 会把 log-mel、编码器输出与
//! prompt id 落盘，交给 `tools/whisper_reference.py` 用 transformers 逐层比较。

use std::path::Path;
use std::time::Instant;

use xime_infer::asr::whisper::Whisper;
use xime_infer::asr::AsrOptions;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: whisper_transcribe <模型目录> <wav> [语种，如 zh]");
        std::process::exit(2);
    }
    let dir = Path::new(&args[1]);
    let wav = Path::new(&args[2]);
    let language = args.get(3).cloned();

    let mut opts = AsrOptions {
        language: language.clone(),
        ..Default::default()
    };
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
            "bf16" => candle_core::DType::BF16,
            "f16" => candle_core::DType::F16,
            _ => candle_core::DType::F32,
        };
    }
    println!(
        "device = {}, dtype = {:?}, 语种 = {:?}",
        opts.device.as_str(),
        opts.dtype,
        opts.language
    );

    let t0 = Instant::now();
    let mut asr = Whisper::from_dir_with(dir, opts)?;
    let cfg = asr.config().clone();
    println!(
        "装载模型: {:.0} ms（d_model {}，编码器 {} 层 / 解码器 {} 层，{} mel）",
        t0.elapsed().as_secs_f64() * 1000.0,
        cfg.d_model,
        cfg.encoder_layers,
        cfg.decoder_layers,
        cfg.num_mel_bins
    );

    let task = std::env::var("XIME_ASR_TASK").unwrap_or_else(|_| "transcribe".into());
    asr.set_task(&task)?;

    let t = Instant::now();
    asr.warmup()?;
    println!(
        "预热: {:.0} ms（task = {task}）",
        t.elapsed().as_secs_f64() * 1000.0
    );

    if let Ok(dump) = std::env::var("XIME_ASR_DUMP") {
        std::fs::create_dir_all(&dump)?;
        let pcm = xime_infer::asr::audio::load_wav_16k(wav, 16_000)?;
        let mel = asr.mel(&pcm)?;
        let enc = asr.encode(&pcm)?;
        // 自动模式要先把检测到的语种写进 prompt，才能与参考实现对齐。
        let detected = if language.is_some() {
            None
        } else {
            asr.detect_language(&pcm)?
        };
        let prompt = asr.prompt_tokens(language.as_deref().or(detected.as_deref()), &task)?;

        let mut bytes = Vec::with_capacity(mel.data.len() * 4);
        for v in &mel.data {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(format!("{dump}/mel_f32.bin"), &bytes)?;
        std::fs::write(
            format!("{dump}/mel_shape.txt"),
            format!("{} {}", mel.mel_bins, mel.frames),
        )?;

        let words = enc.flatten_all()?.to_vec1::<f32>()?;
        let mut bytes = Vec::with_capacity(words.len() * 4);
        for v in &words {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(format!("{dump}/encoder_f32.bin"), &bytes)?;
        std::fs::write(
            format!("{dump}/encoder_shape.txt"),
            format!("{} {}", enc.dims()[1], enc.dims()[2]),
        )?;
        std::fs::write(format!("{dump}/prompt_ids.txt"), format!("{prompt:?}"))?;
        println!(
            "dump → {dump}（mel {:?}，encoder {:?}）",
            mel.frames,
            enc.dims()
        );
    }

    for run in 0..3 {
        let out = asr.transcribe_wav(wav)?;
        println!(
            "run {run}: {:.0} ms（编码 {:.0} + 解码 {:.0}），音频 {:.2} s，\
             {} 个位置，{} 个文本 token，语种 {:?}",
            out.encode_ms + out.decode_ms,
            out.encode_ms,
            out.decode_ms,
            out.audio_ms / 1000.0,
            out.audio_tokens,
            out.text_tokens,
            out.language,
        );
        println!("  文本: {}", out.text);
        if let Ok(ids) = std::env::var("XIME_ASR_DUMP_IDS") {
            std::fs::write(ids, format!("{:?}", out.token_ids))?;
        }
    }

    Ok(())
}
