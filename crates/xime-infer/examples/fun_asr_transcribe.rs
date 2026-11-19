//! Fun-ASR-Nano 冒烟测试：装载、预热、转写一段 WAV，打印耗时与文本。
//!
//! 用法：
//! ```text
//! cargo run --release -p xime-infer --example fun_asr_transcribe -- <模型目录> <wav> [语种]
//! ```
//!
//! 模型目录是 `tools/convert_fun_asr_nano.py` 的产出（含 `config.json` 与
//! `model.safetensors`），或官方 `-hf` 仓库的快照。

use std::path::Path;
use std::time::Instant;

use xime_infer::asr::fun_asr::FunAsrNano;
use xime_infer::asr::AsrOptions;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: fun_asr_transcribe <模型目录> <wav> [语种，如 中文]");
        std::process::exit(2);
    }
    let dir = Path::new(&args[1]);
    let wav = Path::new(&args[2]);
    let language = args.get(3).cloned();

    let mut opts = AsrOptions {
        language,
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
    let mut asr = FunAsrNano::from_dir_with(dir, opts)?;
    println!("装载模型: {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);

    let t = Instant::now();
    asr.warmup()?;
    println!("预热: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);

    // 对拍用：把前处理特征、编码器输出、prompt token 落到磁盘，交给
    // `tools/fun_asr_reference.py` 逐元素比较。
    if let Ok(dump) = std::env::var("XIME_ASR_DUMP") {
        std::fs::create_dir_all(&dump)?;
        let pcm = xime_infer::asr::audio::load_wav_16k(wav, 16_000)?;
        let (feats, frames, dim) = asr.fbank(&pcm)?;
        let audio = asr.encode(&pcm)?;
        let ids = asr.prompt_tokens(Some("中文"))?;

        let mut bytes = Vec::with_capacity(feats.len() * 4);
        for v in &feats {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(format!("{dump}/feats_f32.bin"), &bytes)?;
        std::fs::write(format!("{dump}/feats_shape.txt"), format!("{frames} {dim}"))?;

        let words = audio.flatten_all()?.to_vec1::<f32>()?;
        let mut bytes = Vec::with_capacity(words.len() * 4);
        for v in &words {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(format!("{dump}/audio_f32.bin"), &bytes)?;
        std::fs::write(
            format!("{dump}/audio_shape.txt"),
            format!("{} {}", audio.dims()[0], audio.dims()[1]),
        )?;

        let mut prompt = ids.prefix.clone();
        prompt.extend(&ids.suffix);
        std::fs::write(format!("{dump}/prompt_ids.txt"), format!("{prompt:?}"))?;
        println!(
            "dump → {dump}（feats {frames}×{dim}，audio {:?}，prompt {} token）",
            audio.dims(),
            prompt.len()
        );
    }

    for run in 0..3 {
        let out = asr.transcribe_wav(wav)?;
        println!(
            "run {run}: {:.0} ms（编码 {:.0} + 解码 {:.0}），音频 {:.2} s，\
             {} 个音频 token，{} 个文本 token，RT {:.2}x",
            out.encode_ms + out.decode_ms,
            out.encode_ms,
            out.decode_ms,
            out.audio_ms / 1000.0,
            out.audio_tokens,
            out.text_tokens,
            (out.encode_ms + out.decode_ms) / out.audio_ms,
        );
        println!("  文本: {}", out.text);
        if let Ok(ids) = std::env::var("XIME_ASR_DUMP_IDS") {
            std::fs::write(ids, format!("{:?}", out.token_ids))?;
        }
    }

    Ok(())
}
