//! 识别引擎抽象。
//!
//! 生产实现包 [`xime_infer::asr::Qwen3Asr`]；测试可以塞一个假引擎，
//! 这样状态机、协议、模型目录都能在没有 1.6 GB 权重的情况下测。
//!
//! 引擎有两种工作方式，服务层按能力择优：
//!
//! - **整段**（[`Recognizer::transcribe`]）：录完一次性识别，质量最好；
//! - **流式**（[`Recognizer::encode_block`] + [`Recognizer::decode_window`]）：
//!   录音中逐秒编码、滚动窗口重解码，边说边出字。能力由
//!   [`Recognizer::supports_streaming`] 声明，不支持的引擎退回整段。

use std::path::Path;

use candle_core::{DType, Tensor};
use xime_infer::asr::{AsrOptions, Qwen3Asr};
use xime_infer::DeviceKind;

/// 一段已编码的音频特征，`[tokens, d_model]`。
///
/// 1 秒音频 = 1 个 chunk = 13 个 token；多块用 `Tensor::cat` 沿第 0 维拼。
pub type AudioBlock = Tensor;

/// 一段 16 kHz 单声道 PCM → 文本。
pub trait Recognizer: Send {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String, String>;

    /// 是否支持流式（分块编码 + 窗口重解码）。默认不支持。
    fn supports_streaming(&self) -> bool {
        false
    }

    /// 编码一小段音频（≤1 秒）为特征块。只读，可并发调用。
    fn encode_block(&self, _pcm: &[f32]) -> Result<AudioBlock, String> {
        Err("该引擎不支持分块编码".to_string())
    }

    /// 用一段窗口特征解码。
    ///
    /// `context` 是本次听写中**已定稿**的文本，实现应把它作为解码提示
    /// （Qwen3-ASR 走 prompt 的 system 段），让窗口接缝处的用词连贯。
    fn decode_window(&mut self, _window: &AudioBlock, _context: &str) -> Result<String, String> {
        Err("该引擎不支持窗口解码".to_string())
    }
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

    fn supports_streaming(&self) -> bool {
        true
    }

    fn encode_block(&self, pcm: &[f32]) -> Result<AudioBlock, String> {
        self.asr
            .encode(pcm)
            .map_err(|e| format!("音频编码失败：{e}"))
    }

    fn decode_window(&mut self, window: &AudioBlock, context: &str) -> Result<String, String> {
        // 时长只用于 Transcript 的统计字段，流式侧不需要。
        self.asr
            .decode_embeds(window, Some(context), 0.0)
            .map(|out| out.text)
            .map_err(|e| format!("识别失败：{e}"))
    }
}
