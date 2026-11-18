//! 识别引擎抽象。
//!
//! 生产实现包 [`xime_infer::asr::Qwen3Asr`]；测试可以塞一个假引擎，
//! 这样状态机、协议、模型目录都能在没有 1.6 GB 权重的情况下测。

use std::path::Path;

use candle_core::DType;
use xime_infer::asr::{AsrOptions, Qwen3Asr};
use xime_infer::DeviceKind;

/// 一段 16 kHz 单声道 PCM → 文本。
pub trait Recognizer: Send {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String, String>;
}

/// Qwen3-ASR 实现。
pub struct QwenRecognizer {
    asr: Qwen3Asr,
}

impl QwenRecognizer {
    /// 装载模型目录（阻塞；调用方负责放到后台线程）。
    pub fn load(dir: &Path, device: DeviceKind, dtype: DType) -> Result<Self, String> {
        let options = AsrOptions {
            device,
            dtype,
            ..Default::default()
        };
        let asr =
            Qwen3Asr::from_dir_with(dir, options).map_err(|e| format!("装载模型失败：{e}"))?;
        Ok(Self { asr })
    }
}

impl Recognizer for QwenRecognizer {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String, String> {
        self.asr
            .transcribe(pcm)
            .map(|out| out.text)
            .map_err(|e| format!("识别失败：{e}"))
    }
}
