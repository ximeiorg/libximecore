//! 本地语音转文本推理（跨端核心，Windows / Linux / Android 宿主共用）。
//!
//! 分工边界（对齐 Android xime speech 模块的切法）：
//! - 本 crate 只做**纯推理**：模型注册表、流式识别器封装（sherpa-onnx）。
//! - **音频采集归宿主**（Windows = winxime-server 的 WASAPI，Android =
//!   AudioRecord）；本 crate 不碰麦克风、不做权限。
//! - **模型下载归宿主**（server 已有 ureq/tar 基建）；注册表只提供 URL
//!   与文件角色清单。
//! - 交互语义与 Android `AsrNative` 一致：装载 → 喂 pcm16 音频块 → 查询
//!   partial → 结束时 finalize 取最终文本。
//! - 线程纪律：识别器与流归调用方 worker 线程**独占**（不跨线程共享，
//!   对齐 PluginRuntime 的所有权模型）；推理绝不持有输入法引擎锁。
//!
//! GPU（分包模型）：`cuda` feature 决定一个包**支持什么**——CPU 包不启用
//! feature（`SpeechProvider::Cuda` 变体不存在，编译期挡下误用）；CUDA 包启用
//! feature 并在构建时用 `SHERPA_ONNX_LIB_DIR` 指向 CUDA 预编译包（含
//! onnxruntime CUDA EP + cudnn/cublas 全家，与 CPU 包同 ABI）的 lib 目录、
//! 用独立 target 目录构建（两 flavor 的 onnxruntime.dll 同名，混用会互踩）。
//! 最终程序按包型各自打包分发（CPU 包 / CUDA 包）。

mod recognizer;
mod registry;

pub use recognizer::{SpeechConfig, SpeechError, SpeechProvider, StreamingRecognizer};
pub use registry::{AsrModelProfile, AsrModelRegistry};

/// sherpa-onnx C 库版本（诊断/日志用）。
pub fn sherpa_version() -> &'static str {
    sherpa_onnx::version()
}

/// 当前构建是否带 CUDA 支持（`cuda` feature，即 CUDA 分包）。
pub fn cuda_supported() -> bool {
    cfg!(feature = "cuda")
}
