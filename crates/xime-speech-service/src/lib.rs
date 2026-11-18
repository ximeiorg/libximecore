//! 本地语音识别服务内核。
//!
//! 三层：
//!
//! - [`service`]：模型目录 + 引擎生命周期 + 一次会话的状态机（无平台代码）；
//! - [`capture`]：麦克风采集（cpal），录音进程唯一；
//! - [`transport`]：Unix socket 上的 JSON-line 协议（[`protocol`]）。
//!
//! 进程入口在 `xime-speechd`；宿主（XimeYi / XimeChe）负责按平台托管这个进程。

//! 客户端（输入法 / 设置页）关掉默认 feature，只链 `protocol` 与 `transport`，
//! 不会把 candle 与 cpal 带进 UI 进程。

pub mod protocol;
pub mod transport;

#[cfg(feature = "server")]
pub mod capture;
#[cfg(feature = "server")]
pub mod engine;
#[cfg(feature = "server")]
pub mod registry;
#[cfg(feature = "server")]
pub mod service;

#[cfg(feature = "server")]
pub use candle_core::DType;
#[cfg(feature = "server")]
pub use engine::Recognizer;
pub use protocol::{ModelInfo, Request, Response, Status, PROTOCOL_VERSION};
#[cfg(feature = "server")]
pub use registry::{format_size, Entry, Registry};
#[cfg(feature = "server")]
pub use service::{ServiceConfig, SpeechService};
#[cfg(feature = "server")]
pub use xime_infer::DeviceKind;
