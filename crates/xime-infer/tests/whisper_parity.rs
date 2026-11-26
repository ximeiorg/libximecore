//! Whisper 端到端对拍（需要真实权重，默认 `#[ignore]`）。
//!
//! ```text
//! # 两代模型分别跑（small 244M / large-v3-turbo 809M，CPU 上 turbo 编码约 9 s）
//! XIME_WHISPER_MODEL_DIR=/srv/share/TDAS/models/openai/whisper-small \
//!   cargo test --release -p xime-infer --test whisper_parity -- --ignored --test-threads=1
//!
//! XIME_WHISPER_MODEL_DIR=/srv/share/TDAS/models/openai/whisper-large-v3-turbo \
//!   cargo test --release -p xime-infer --test whisper_parity -- --ignored --test-threads=1
//! ```
//!
//! 测试共享**同一个**识别器实例（cargo test 默认多线程；每个测试各装一份模型会
//! 把内存/显存吃满，且 `Whisper` 本身有进程级单实例闸门）。
//!
//! 期望文本来自 transformers `WhisperForConditionalGeneration.generate` 的贪心解码
//! （同一份权重、同一段 jfk.wav）：
//!
//! | 模型 | 文本 |
//! | --- | --- |
//! | small | `And so my fellow Americans, ask not what your country …` |
//! | large-v3-turbo | `And so, my fellow Americans, ask not what your country …` |
//!
//! 两者的标点略有差异（turbo 多一个逗号），是模型本身的差别，不是实现问题；
//! 断言只比「关键短语」和语种，避免把某一代的标点选择写成契约。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use xime_infer::asr::whisper::{Whisper, MAX_AUDIO_SECS};
use xime_infer::asr::AsrOptions;
use xime_infer::{DeviceKind, InferError};

static MODEL: OnceLock<Mutex<Whisper>> = OnceLock::new();

fn model_dir() -> PathBuf {
    let dir = std::env::var("XIME_WHISPER_MODEL_DIR").expect(
        "需要 XIME_WHISPER_MODEL_DIR 指向 whisper 模型目录\
         （含 config.json / generation_config.json / tokenizer.json / model.safetensors）",
    );
    PathBuf::from(dir)
}

fn wav(name: &str, default_rel: &str) -> PathBuf {
    let p = match std::env::var(name) {
        Ok(v) => PathBuf::from(v),
        Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(default_rel),
    };
    assert!(p.exists(), "WAV 不存在：{}", p.display());
    p
}

fn shared() -> &'static Mutex<Whisper> {
    MODEL.get_or_init(|| {
        let mut opts = AsrOptions::default();
        if let Some(device) = device_from_env() {
            opts.device = device;
            if device.as_str() == "metal" {
                opts.dtype = candle_core::DType::F16;
            }
        }
        let mut asr = Whisper::from_dir_with(&model_dir(), opts).expect("装载模型失败");
        asr.warmup().expect("预热失败");
        Mutex::new(asr)
    })
}

fn device_from_env() -> Option<DeviceKind> {
    let v = std::env::var("XIME_ASR_DEVICE").ok()?;
    let dev = match v.as_str() {
        "cpu" => DeviceKind::Cpu,
        #[cfg(all(target_os = "macos", feature = "metal"))]
        "metal" => DeviceKind::Metal,
        #[cfg(feature = "cuda")]
        "cuda" => DeviceKind::Cuda,
        other => panic!("XIME_ASR_DEVICE={other} 与本次构建的 feature 不匹配"),
    };
    Some(dev)
}

#[test]
#[ignore = "需要真实权重"]
fn transcribes_jfk_with_auto_language() {
    let wav = wav("XIME_WHISPER_TEST_WAV", "target/asr-test/jfk.wav");
    let mut asr = shared().lock().unwrap();
    let out = asr.transcribe_wav(&wav).unwrap();

    assert_eq!(out.language.as_deref(), Some("en"), "自动语种检测");
    let lower = out.text.to_lowercase();
    for phrase in [
        "my fellow americans",
        "ask not what your country can do for you",
        "ask what you can do for your country",
    ] {
        assert!(lower.contains(phrase), "缺少 `{phrase}`：{}", out.text);
    }
    assert_eq!(out.audio_tokens, 1500, "编码器固定 30 s / 1500 位置");
    assert!(out.encode_ms > 0.0 && out.decode_ms > 0.0);
    // 静音补齐不该产生幻觉：输出不应含 <|...|> 残留或乱码替换符
    assert!(!out.text.contains("<|") && !out.text.contains('\u{fffd}'));
}

#[test]
#[ignore = "需要真实权重"]
fn honours_forced_language() {
    let wav = wav("XIME_WHISPER_TEST_WAV", "target/asr-test/jfk.wav");
    let mut asr = shared().lock().unwrap();
    let out = asr
        .transcribe_with_language(
            &xime_infer::asr::audio::load_wav_16k(&wav, 16_000).unwrap(),
            Some("English".into()),
        )
        .unwrap();
    assert_eq!(out.language.as_deref(), Some("en"));
    assert!(out.text.to_lowercase().contains("fellow americans"));
}

#[test]
#[ignore = "需要真实权重"]
fn unknown_language_is_reported() {
    let wav = wav("XIME_WHISPER_TEST_WAV", "target/asr-test/jfk.wav");
    let pcm = xime_infer::asr::audio::load_wav_16k(&wav, 16_000).unwrap();
    let mut asr = shared().lock().unwrap();
    let err = asr
        .transcribe_with_language(&pcm, Some("klingon".into()))
        .unwrap_err();
    match err {
        InferError::Config(msg) => assert!(msg.contains("klingon"), "{msg}"),
        other => panic!("期望 Config 错误，得到 {other:?}"),
    }
}

#[test]
#[ignore = "需要真实权重"]
fn rejects_empty_and_overlong_audio() {
    let mut asr = shared().lock().unwrap();
    assert!(matches!(
        asr.transcribe(&[]).unwrap_err(),
        InferError::EmptyAudio
    ));

    let too_long = vec![0.0f32; (MAX_AUDIO_SECS as usize + 1) * 16_000];
    match asr.transcribe(&too_long).unwrap_err() {
        InferError::AudioTooLong { max, .. } => assert_eq!(max, MAX_AUDIO_SECS),
        other => panic!("期望 AudioTooLong，得到 {other:?}"),
    }
}

#[test]
#[ignore = "需要真实权重"]
fn second_instance_is_blocked_by_gate() {
    // 共享实例已占着闸门；再装一个必须被挡下（GPU 显存/内存限制）。
    let err = match Whisper::from_dir(&model_dir()) {
        Ok(_) => panic!("第二个实例竟然装载成功了"),
        Err(e) => e,
    };
    assert!(matches!(err, InferError::AsrBusy(_)), "{err:?}");
}

/// log-mel 前处理与老版 transformers 存的 mel 滤波器组逐点对齐。
///
/// 新版 `preprocessor_config.json` 已删掉 `mel_filters`，这时跳过；whisper-small
/// 这份（transformers 4.27 生成）还带着 80×201 的表，正好当独立参考。
#[test]
#[ignore = "需要真实权重"]
fn mel_filters_match_stored_reference() {
    let dir = model_dir();
    let path = dir.join("preprocessor_config.json");
    let text = std::fs::read_to_string(&path).expect("读不到 preprocessor_config.json");
    let cfg: serde_json::Value = serde_json::from_str(&text).unwrap();
    let Some(filters) = cfg.get("mel_filters").and_then(|v| v.as_array()) else {
        eprintln!(
            "{} 里没有 mel_filters（新版 transformers 不存了），跳过",
            path.display()
        );
        return;
    };

    let mel_bins = filters.len();
    let n_freq = filters[0].as_array().unwrap().len();
    assert_eq!(n_freq, 201);

    let pcm = vec![0.0f32; 16_000];
    let asr = shared().lock().unwrap();
    let computed = asr.mel_frontend().mel_filters();
    assert_eq!(computed.len(), mel_bins * n_freq);

    let mut max_diff = 0.0f32;
    for (m, row) in filters.iter().enumerate() {
        let row = row.as_array().unwrap();
        for (f, want) in row.iter().enumerate() {
            let want = want.as_f64().unwrap() as f32;
            let got = computed[m * n_freq + f];
            max_diff = max_diff.max((got - want).abs());
        }
    }
    assert!(max_diff < 2e-6, "mel 滤波器最大误差 {max_diff}");
    // 静音 mel 的形状也要对：80 或 128 维 × 3000 帧
    let mel = asr.mel(&pcm).unwrap();
    assert_eq!(mel.frames, 3000);
    assert_eq!(mel.mel_bins, mel_bins);
}

/// 语种检测与 `generation_config.json` 的语种表一致（至少 jfk 这段是英语）。
#[test]
#[ignore = "需要真实权重"]
fn detect_language_returns_code() {
    let wav = wav("XIME_WHISPER_TEST_WAV", "target/asr-test/jfk.wav");
    let pcm = xime_infer::asr::audio::load_wav_16k(&wav, 16_000).unwrap();
    let mut asr = shared().lock().unwrap();
    assert_eq!(asr.detect_language(&pcm).unwrap().as_deref(), Some("en"));
}
