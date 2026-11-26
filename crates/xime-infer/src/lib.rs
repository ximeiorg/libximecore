//! 本地模型推理（candle）。
//!
//! 分工边界：
//! - 本 crate 只做**推理**：模型装载、前处理、解码。模型下载/解压/校验归宿主。
//! - 不依赖 librime，不碰输入法引擎锁；识别器由调用方 worker 线程独占使用。
//! - 后端按 feature 编译：默认 CPU；macOS 可开 `metal`；Windows/Linux 可开 `cuda`。
//!   两 flavor 的 CUDA 依赖同名，需独立 target 目录构建。
//!
//! 两个模型族：
//! - [`stroke`]：笔画手写识别（StrokeTransformer，小 transformer）。
//! - [`asr`]：语音识别，按模型分目录：
//!   - [`asr::qwen3_asr`]：Qwen3-ASR（Whisper 式 log-mel + 18 层音频塔 + Qwen3 解码器）；
//!   - [`asr::fun_asr`]：Fun-ASR-Nano（Kaldi fbank + 70 层 SAN-M + Qwen3-0.6B 解码器）；
//!   - [`asr::whisper`]：Whisper（30 s 定长 log-mel + 编码器-解码器 + 交叉注意力）。
//!
//!   各家族**各自一套** config / 前处理 / 编码器 / 文本解码器 / 分词器，只在
//!   [`asr::AsrOptions`] / [`asr::Transcript`] 这类与模型无关的类型上共用。

// 包型 feature 是互斥的：GPU flavor 与平台一一对应，误组合在编译期挡下。
//
// | 平台 | 后端 feature | 说明 |
// | --- | --- | --- |
// | macOS | `metal`（可选） | Apple Silicon / Intel Mac 的 GPU |
// | Windows / Linux | `cuda`（可选） | 仅 NVIDIA；其他 GPU 走 CPU |
// | 任意 | 无（默认） | CPU；macOS 可加 `accelerate` |
#[cfg(all(feature = "metal", feature = "cuda"))]
compile_error!("xime-infer：`metal` 与 `cuda` 是互斥的包型 feature，不能同时开启");
#[cfg(all(feature = "metal", not(target_os = "macos")))]
compile_error!("xime-infer：`metal` 仅支持 macOS");
#[cfg(all(feature = "cuda", target_os = "macos"))]
compile_error!("xime-infer：CUDA 后端不支持 macOS（无 NVIDIA 驱动），请用 `metal`");
#[cfg(all(feature = "accelerate", not(target_os = "macos")))]
compile_error!("xime-infer：`accelerate` 仅支持 macOS");

mod device;

pub mod asr;
pub mod stroke;

pub use device::DeviceKind;

/// 推理统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum InferError {
    /// 后端初始化失败（驱动/运行库缺失）。
    #[error("推理后端初始化失败：{0}")]
    Device(String),
    /// 模型目录里找不到约定文件。
    #[error("模型文件缺失：{0}")]
    MissingFile(String),
    /// 读文件失败。
    #[error("模型文件读取失败：{path}：{source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    /// 文本文件不是合法 UTF-8。
    #[error("模型文件不是 UTF-8：{0}")]
    Text(String),
    /// 标签 JSON 结构不合法。
    #[error("标签文件解析失败：{0}")]
    Labels(#[from] serde_json::Error),
    /// 模型配置（config.json）字段缺失或取值非法。
    #[error("模型配置不合法：{0}")]
    Config(String),
    /// 音频解码/重采样失败。
    #[error("音频处理失败：{0}")]
    Audio(String),
    /// 分词器装载或解码失败。
    #[error("分词器失败：{0}")]
    Tokenizer(String),
    /// 标签数与模型输出类别数不一致（模型/标签不是同一版本）。
    #[error("标签数量 {actual} 与模型类别数 {expected} 不一致")]
    LabelCount { actual: usize, expected: usize },
    /// candle 张量/模型错误。
    #[error("张量错误：{0}")]
    Candle(#[from] candle_core::Error),
    /// 没有任何落笔点。
    #[error("没有可识别的笔画")]
    EmptyInput,
    /// 没有任何音频采样点。
    #[error("没有音频采样点")]
    EmptyAudio,
    /// 同一进程已有 ASR 实例（各家族共用同一道闸门）。
    #[error("已有 ASR 实例在运行：{0}")]
    AsrBusy(String),
    /// 音频超过模型单次可处理的时长（各家族都是 30 秒窗口）。
    #[error("音频 {secs:.1} 秒超过单次上限 {max:.0} 秒")]
    AudioTooLong { secs: f32, max: f32 },
}
