//! Xime JS 插件系统（QuickJS，契约对齐 xime 3.0 Android `JsScriptRuntime`）。
//!
//! 插件包（.xipk）为 zip 容器：`manifest.json`（元数据，兼容旧 `manifest.yaml`）+
//! `entry` 指定的 JS 入口脚本（默认 main.js，IIFE 把导出对象挂到 `globalThis.plugin`，
//! 分组命名空间：emoji / clipboardSync / backup / settings / transform / panel /
//! speech / events），可选 `resources/`（资源，宿主只给路径）与 `libs/`（受限
//! require 的模块）。
//! 沙箱屏蔽 eval/Function，插件只能访问注入的 `host` 白名单 API；
//! 契约调用带硬超时（15ms transform / 5s 回调 / 180s 业务），超时熔断降级。

pub mod capabilities;
mod manifest;
mod runtime;

pub mod manager;

pub use capabilities::{
    ClipboardSyncCapabilities, EmojiCapabilities, PluginCapabilities, SpeechCapabilities,
    ToolCapabilities,
};
pub use manifest::{NetworkDecl, PluginManifest, PluginType, ToolbarButton};
pub use runtime::{
    BackupUploadResult, CandidateTransformCircuitBreaker, CandidateTransformItem,
    CandidateTransformOutcome, EmojiItem, EmojiLayout, PluginRuntime, RemoteBackupEntry,
    RuntimeError, RuntimeResult, SettingField,
};

pub use manager::{PluginManager, PluginRecord, PluginRecordState};
