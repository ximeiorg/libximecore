//! 识别引擎抽象。
//!
//! 生产实现有两个：`candle` 侧的 [`xime_infer::asr::Qwen3Asr`]（流式友好）与
//! [`xime_infer::asr::fun_asr::FunAsrNano`]（中文方言更强，只能整段识别）。
//! 用 [`load`] 按模型目录的 `config.json` 自动选，宿主不需要知道型号差异。
//! 测试可以塞一个假引擎，这样状态机、协议、模型目录都能在没有 1.6 GB 权重的情况下测。
//!
//! 引擎有两种工作方式，服务层按能力择优：
//!
//! - **整段**（[`Recognizer::transcribe`]）：录完一次性识别，质量最好；
//! - **流式**（[`Recognizer::encode_block`] + [`Recognizer::decode_window`]）：
//!   录音中逐秒编码、滚动窗口重解码，边说边出字。能力由
//!   [`Recognizer::supports_streaming`] 声明，不支持的引擎退回整段。

use std::path::Path;

use candle_core::{DType, Tensor};
use xime_infer::asr::{fun_asr::FunAsrNano, AsrFamily, AsrOptions, Qwen3Asr};
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

/// 按模型目录装载引擎：看 `config.json` 是哪个家族。
///
/// 调用方（`spawn_load`）在后台线程里跑，装载失败只写进状态，不影响守护进程。
pub fn load(dir: &Path, device: DeviceKind, dtype: DType) -> Result<Box<dyn Recognizer>, String> {
    let family = AsrFamily::detect(dir).map_err(|e| format!("读取模型配置失败：{e}"))?;
    match family {
        AsrFamily::Qwen3Asr => Ok(Box::new(QwenRecognizer::load(dir, device, dtype)?)),
        AsrFamily::FunAsrNano => Ok(Box::new(FunAsrRecognizer::load(dir, device, dtype)?)),
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

/// Fun-ASR-Nano 实现。
///
/// **只做整段识别**：它的前处理要跨帧拼 LFR、编码器是整段双向注意力，分块编码
/// 与整段编码不等价（详见 `xime_infer::asr::fun_asr` 的模块文档）。服务层靠
/// [`Recognizer::supports_streaming`] 判定，录音期间不出预览，松手后一次性上屏。
pub struct FunAsrRecognizer {
    asr: FunAsrNano,
}

impl FunAsrRecognizer {
    pub fn load(dir: &Path, device: DeviceKind, dtype: DType) -> Result<Self, String> {
        let options = AsrOptions {
            device,
            dtype,
            ..Default::default()
        };
        let asr =
            FunAsrNano::from_dir_with(dir, options).map_err(|e| format!("装载模型失败：{e}"))?;
        Ok(Self { asr })
    }
}

impl Recognizer for FunAsrRecognizer {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String, String> {
        self.asr
            .transcribe(pcm)
            .map(|out| out.text)
            .map_err(|e| format!("识别失败：{e}"))
    }
}
