//! Qwen3-ASR 端到端对拍。
//!
//! 需要真实权重（约 1.6 GB），默认 `#[ignore]`：
//!
//! ```text
//! # CPU（推荐；开 --features accelerate 才快）
//! XIME_ASR_MODEL_DIR=/path/to/Qwen3-ASR-0.6B \
//!   cargo test --release -p xime-infer --features accelerate \
//!     --test asr_parity -- --ignored --test-threads=1
//!
//! # Metal（f16）：显存常驻约 1.6 GB，务必单线程、单实例
//! XIME_ASR_MODEL_DIR=... XIME_ASR_DEVICE=metal \
//!   cargo test --release -p xime-infer --features metal \
//!     --test asr_parity -- --ignored --test-threads=1
//! ```
//!
//! 测试共享**同一个**识别器实例：cargo test 默认多线程，每个测试各装一份模型会在
//! macOS 上同时申请 4×1.6 GB GPU 内存，触发 `IOGPUGroupMemory` 内核断言。
//! `Qwen3Asr` 本身也有进程级单实例闸门，第二个实例会返回 `AsrBusy`。
//!
//! 预期文本来自 transformers 官方实现（`Qwen3ASRForConditionalGeneration` + 同一份权重）
//! 的贪心解码结果，token 级一致；这里只断言文本，不锁死语种判定。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use xime_infer::asr::{AsrOptions, Qwen3Asr};

static MODEL: OnceLock<Mutex<Qwen3Asr>> = OnceLock::new();

fn model_dir() -> PathBuf {
    let dir = std::env::var("XIME_ASR_MODEL_DIR").expect(
        "需要 XIME_ASR_MODEL_DIR 指向 Qwen3-ASR 模型目录（含 config.json 与 model.safetensors）",
    );
    PathBuf::from(dir)
}

fn wav(name: &str, default_rel: &str) -> PathBuf {
    // 默认路径相对仓库根（cargo test 的 cwd 是 crate 目录）。
    let p = match std::env::var(name) {
        Ok(v) => PathBuf::from(v),
        Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(default_rel),
    };
    assert!(p.exists(), "WAV 不存在：{}", p.display());
    p
}

/// 共享的唯一实例。默认后端见 `AsrOptions::default`（CUDA 包自动探测，其余 CPU）；
/// `XIME_ASR_DEVICE=cpu|metal|cuda` 可覆盖。
fn shared() -> &'static Mutex<Qwen3Asr> {
    MODEL.get_or_init(|| {
        #[allow(unused_mut)] // 默认后端之外通常不改
        let mut opts = AsrOptions::default();
        if let Some(device) = device_from_env() {
            opts.device = device;
            if device.as_str() == "metal" {
                // Metal 上 f16 与 f32 输出一致、显存减半
                opts.dtype = candle_core::DType::F16;
            }
        }
        let mut asr = Qwen3Asr::from_dir_with(&model_dir(), opts).expect("装载模型失败");
        asr.warmup().expect("预热失败");
        Mutex::new(asr)
    })
}

/// `XIME_ASR_DEVICE=cpu|metal|cuda` 覆盖默认后端。
fn device_from_env() -> Option<xime_infer::DeviceKind> {
    let v = std::env::var("XIME_ASR_DEVICE").ok()?;
    let dev = match v.as_str() {
        "cpu" => xime_infer::DeviceKind::Cpu,
        #[cfg(all(target_os = "macos", feature = "metal"))]
        "metal" => xime_infer::DeviceKind::Metal,
        #[cfg(feature = "cuda")]
        "cuda" => xime_infer::DeviceKind::Cuda,
        other => panic!("XIME_ASR_DEVICE={other} 与本次构建的 feature 不匹配"),
    };
    Some(dev)
}

fn with_asr<T>(f: impl FnOnce(&mut Qwen3Asr) -> T) -> T {
    let mut guard = shared().lock().unwrap();
    f(&mut guard)
}

#[test]
#[ignore = "需要真实权重，设 XIME_ASR_MODEL_DIR"]
fn transcribe_jfk() {
    let wav = wav("XIME_ASR_WAV", "target/asr-test/jfk.wav");
    let out = with_asr(|asr| asr.transcribe_wav(&wav).expect("识别失败"));

    assert_eq!(
        out.language.as_deref(),
        Some("English"),
        "语种判定：{:?}",
        out.language
    );
    assert!(
        out.text
            .contains("ask not what your country can do for you"),
        "识别结果不符：{}",
        out.text
    );
    // 11 秒音频 → 143 个音频 token（13 × 11），与官方 `_get_feat_extract_output_lengths` 一致
    assert_eq!(out.audio_tokens, 143);
    assert!(out.encode_ms > 0.0 && out.decode_ms > 0.0);
}

#[test]
#[ignore = "需要真实权重，设 XIME_ASR_MODEL_DIR 与 XIME_ASR_WAV_ZH"]
fn transcribe_chinese_forced_language() {
    let wav = wav("XIME_ASR_WAV_ZH", "target/asr-test/zh.wav");
    let pcm = xime_infer::asr::audio::load_wav_16k(&wav, 16_000).unwrap();
    let out = with_asr(|asr| {
        asr.transcribe_with_language(&pcm, Some("Chinese".into()))
            .expect("识别失败")
    });

    assert_eq!(out.language.as_deref(), Some("Chinese"));
    assert_eq!(out.text, "今天天气不错，我们出去走走吧。");
    // 强制语种时 prompt 预填了 `<asr_text>`，生成里不再有语种前缀
    assert!(!out.token_ids.contains(&151704));
}

#[test]
#[ignore = "需要真实权重，设 XIME_ASR_MODEL_DIR"]
fn transcribe_chinese_48k_resampled() {
    let wav = wav("XIME_ASR_WAV_ZH48", "target/asr-test/zh48.wav");
    let out = with_asr(|asr| asr.transcribe_wav(&wav).expect("识别失败"));
    assert_eq!(out.text, "今天天气不错，我们出去走走吧。");
    // 重采样后帧数按 16 kHz 计算，音频时长不变
    assert!((out.audio_ms - 3279.0).abs() < 60.0, "{}", out.audio_ms);
}

#[test]
#[ignore = "需要真实权重，设 XIME_ASR_MODEL_DIR"]
fn rejects_audio_longer_than_30s() {
    let pcm = vec![0.0f32; 16_000 * 31];
    let err = with_asr(|asr| asr.transcribe(&pcm).unwrap_err());
    assert!(matches!(err, xime_infer::InferError::AudioTooLong { .. }));
}

#[test]
#[ignore = "需要真实权重，设 XIME_ASR_MODEL_DIR"]
fn empty_audio_is_rejected() {
    let err = with_asr(|asr| asr.transcribe(&[]).unwrap_err());
    assert!(matches!(err, xime_infer::InferError::EmptyAudio));
}

#[test]
#[ignore = "需要真实权重，设 XIME_ASR_MODEL_DIR"]
fn second_instance_is_rejected() {
    let _hold = shared(); // 确保第一个实例存在
    let err = match Qwen3Asr::from_dir(&model_dir()) {
        Ok(_) => panic!("第二个实例不应该装载成功"),
        Err(e) => e,
    };
    assert!(matches!(err, xime_infer::InferError::AsrBusy(_)));
}
