//! 本地模型推理（candle）。
//!
//! 分工边界（与 `xime-speech` 一致）：
//! - 本 crate 只做**推理**：模型装载、前处理、top-k。模型下载/解压归宿主。
//! - 不依赖 librime，不碰输入法引擎锁；识别器由调用方 worker 线程独占使用。
//! - 后端按 feature 编译：默认 CPU；macOS 可开 `metal`；Windows/Linux 可开 `cuda`。
//!   两 flavor 的 CUDA 依赖同名，需独立 target 目录构建（见 `xime-speech` 的分包纪律）。

// 包型 feature 是互斥的：GPU flavor 与平台一一对应，误组合在编译期挡下。
#[cfg(all(feature = "metal", feature = "cuda"))]
compile_error!("xime-infer：`metal` 与 `cuda` 是互斥的包型 feature，不能同时开启");
#[cfg(all(feature = "metal", not(target_os = "macos")))]
compile_error!("xime-infer：`metal` 仅支持 macOS");

mod device;

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
    /// 标签数与模型输出类别数不一致（模型/标签不是同一版本）。
    #[error("标签数量 {actual} 与模型类别数 {expected} 不一致")]
    LabelCount { actual: usize, expected: usize },
    /// candle 张量/模型错误。
    #[error("张量错误：{0}")]
    Candle(#[from] candle_core::Error),
    /// 没有任何落笔点。
    #[error("没有可识别的笔画")]
    EmptyInput,
}
