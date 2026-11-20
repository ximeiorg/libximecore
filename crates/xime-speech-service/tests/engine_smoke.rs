//! 真实权重下的引擎派发验证（默认 `#[ignore]`）。
//!
//! 覆盖的是 [`xime_speech_service::engine::load`] 这一层：只给一个模型目录，它要
//! 自己从 `config.json` 认出是 Fun-ASR 还是 Qwen3-ASR，并返回能用的
//! [`Recognizer`]。单元测试用的是假引擎，验不到这条真实路径。
//!
//! 两族的目录都能跑（环境变量名沿用历史的 `XIME_FUNASR_*`，指向 Qwen3 目录同样有效）。
//!
//! ```text
//! XIME_FUNASR_MODEL_DIR=~/.config/xime/models/fun-asr-nano-2512 \
//! XIME_FUNASR_WAV=/path/to/zh.wav \
//!   cargo test --release -p xime-speech-service --test engine_smoke -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use candle_core::DType;
use xime_infer::DeviceKind;
use xime_speech_service::engine;

fn model_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("XIME_FUNASR_MODEL_DIR")
            .expect("需要 XIME_FUNASR_MODEL_DIR 指向转换后的模型目录"),
    )
}

fn wav() -> Option<PathBuf> {
    std::env::var("XIME_FUNASR_WAV").ok().map(PathBuf::from)
}

/// 装载 + 转写：证明「目录 → 引擎 → 文本」这条链在真实权重下是通的。
///
/// 两族都能用：`AsrFamily::detect` 认出是哪一族，再按该族应有的流式能力断言。
/// 之前这里写死「不支持流式」，拿 Qwen3 目录跑会误报失败。
#[test]
#[ignore = "需要真实权重，设 XIME_FUNASR_MODEL_DIR"]
fn fun_asr_dir_dispatches_and_transcribes() {
    let dir = model_dir();
    let family = xime_infer::asr::AsrFamily::detect(&dir).expect("家族识别失败");
    let mut recognizer = engine::load(&dir, DeviceKind::Cpu, DType::F32).expect("引擎装载失败");

    match family {
        // Fun-ASR 的 LFR 跨帧 + 整段注意力决定了它没法分块：服务层据此在录音期间
        // 不出预览，松手后一次上屏。
        xime_infer::asr::AsrFamily::FunAsrNano => {
            assert!(!recognizer.supports_streaming(), "Fun-ASR 应声明不支持流式")
        }
        xime_infer::asr::AsrFamily::Qwen3Asr => {
            assert!(recognizer.supports_streaming(), "Qwen3-ASR 应支持流式")
        }
    }

    let Some(path) = wav() else {
        println!("未设 XIME_FUNASR_WAV，仅验证装载");
        return;
    };
    let pcm = xime_infer::asr::audio::load_wav_16k(Path::new(&path), 16_000).expect("读 WAV 失败");
    let text = recognizer.transcribe(&pcm).expect("识别失败");
    println!("识别结果：{text}");
    assert!(!text.trim().is_empty(), "识别结果为空");
}
