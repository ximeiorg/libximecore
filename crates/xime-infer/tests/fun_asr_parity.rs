//! Fun-ASR-Nano 端到端对拍。
//!
//! 需要真实权重（约 1.6 GB，且要先从 ModelScope 的 `model.pt` 转换），默认 `#[ignore]`：
//!
//! ```text
//! # 1) 先转换（见 tools/convert_fun_asr_nano.py 与 README）
//! # 2) CPU
//! XIME_FUNASR_MODEL_DIR=/path/to/fun-asr-nano \
//!   cargo test --release -p xime-infer --test fun_asr_parity -- --ignored --test-threads=1
//! ```
//!
//! 与 `asr_parity.rs` 一样共享**同一个**识别器实例：进程级闸门会挡下第二个实例。
//!
//! 这里刻意**不锁死识别文本**——逐字对拍靠 `tools/fun_asr_reference.py` 跑参考实现
//! （transformers 的 `FunAsrNanoForConditionalGeneration`，吃同一份 safetensors）：
//! 它会逐层比前处理 / 编码器输出，并把生成的 token id 与 Rust 侧对齐。实测（官方
//! `example/zh.mp3`、`en.mp3`）token 序列逐位相同，见 crate README。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use xime_infer::asr::fun_asr::{frontend::FbankFrontend, FunAsrNano};
use xime_infer::asr::{AsrOptions, Transcript};

static MODEL: OnceLock<Mutex<FunAsrNano>> = OnceLock::new();

fn model_dir() -> PathBuf {
    let dir = std::env::var("XIME_FUNASR_MODEL_DIR").expect(
        "需要 XIME_FUNASR_MODEL_DIR 指向转换后的 Fun-ASR-Nano 目录（含 config.json 与 model.safetensors）",
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

fn shared() -> &'static Mutex<FunAsrNano> {
    MODEL.get_or_init(|| {
        let mut opts = AsrOptions::default();
        if let Some(device) = device_from_env() {
            opts.device = device;
        }
        let mut asr = FunAsrNano::from_dir_with(&model_dir(), opts).expect("装载模型失败");
        asr.warmup().expect("预热失败");
        Mutex::new(asr)
    })
}

fn device_from_env() -> Option<xime_infer::DeviceKind> {
    let v = std::env::var("XIME_FUNASR_DEVICE").ok()?;
    let dev = match v.as_str() {
        "cpu" => xime_infer::DeviceKind::Cpu,
        #[cfg(feature = "cuda")]
        "cuda" => xime_infer::DeviceKind::Cuda,
        other => panic!("XIME_FUNASR_DEVICE={other} 与本次构建的 feature 不匹配"),
    };
    Some(dev)
}

fn with_asr<T>(f: impl FnOnce(&mut FunAsrNano) -> T) -> T {
    let mut guard = shared().lock().unwrap();
    f(&mut guard)
}

/// 与编码器同构的帧数计算，用来核对 `audio_tokens`。
fn expected_tokens(asr: &FunAsrNano, pcm: &[f32]) -> usize {
    let cfg = &asr.config().frontend;
    let front = FbankFrontend::new(
        cfg.sample_rate,
        asr.config().audio.num_mel_bins,
        cfg.frame_length_ms,
        cfg.frame_shift_ms,
        cfg.lfr_m,
        cfg.lfr_n,
    )
    .unwrap();
    front.lfr_frames(pcm.len())
}

#[test]
#[ignore = "需要真实权重，设 XIME_FUNASR_MODEL_DIR"]
fn transcribe_chinese() {
    let path = wav("XIME_FUNASR_WAV", "target/asr-test/zh.wav");
    let pcm = xime_infer::asr::audio::load_wav_16k(&path, 16_000).unwrap();
    let out: Transcript = with_asr(|asr| {
        let expected = expected_tokens(asr, &pcm);
        let out = asr.transcribe(&pcm).expect("识别失败");
        assert_eq!(
            out.audio_tokens, expected,
            "音频 token 数应为 ceil(mel帧/6)"
        );
        out
    });

    assert!(!out.is_empty(), "识别结果为空");
    // 纯文本输出：不该出现 Qwen3-ASR 的语种头标记（`<asr_text>` = 151704）
    assert!(
        !out.token_ids.contains(&151704),
        "输出里混入了 asr_text 标记"
    );
    assert!(out.encode_ms > 0.0 && out.decode_ms > 0.0);
}

#[test]
#[ignore = "需要真实权重，设 XIME_FUNASR_MODEL_DIR"]
fn forced_language_goes_into_prompt() {
    let path = wav("XIME_FUNASR_WAV", "target/asr-test/zh.wav");
    let pcm = xime_infer::asr::audio::load_wav_16k(&path, 16_000).unwrap();
    let out = with_asr(|asr| {
        asr.transcribe_with_language(&pcm, Some("zh".into()))
            .expect("识别失败")
    });
    // Fun-ASR 的语种不是从输出里解析的，而是调用方指定的
    assert_eq!(out.language.as_deref(), Some("中文"));
}

#[test]
#[ignore = "需要真实权重，设 XIME_FUNASR_MODEL_DIR"]
fn rejects_audio_longer_than_30s() {
    let pcm = vec![0.0f32; 16_000 * 31];
    let err = with_asr(|asr| asr.transcribe(&pcm).unwrap_err());
    assert!(matches!(err, xime_infer::InferError::AudioTooLong { .. }));
}

#[test]
#[ignore = "需要真实权重，设 XIME_FUNASR_MODEL_DIR"]
fn empty_audio_is_rejected() {
    let err = with_asr(|asr| asr.transcribe(&[]).unwrap_err());
    assert!(matches!(err, xime_infer::InferError::EmptyAudio));
}

#[test]
#[ignore = "需要真实权重，设 XIME_FUNASR_MODEL_DIR"]
fn second_instance_is_rejected() {
    let _hold = shared();
    let err = match FunAsrNano::from_dir(&model_dir()) {
        Ok(_) => panic!("第二个实例不应该装载成功"),
        Err(e) => e,
    };
    assert!(matches!(err, xime_infer::InferError::AsrBusy(_)));
}
