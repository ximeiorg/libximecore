//! Xime Lua 插件系统。
//!
//! 插件包（.xipk）为 zip 容器：`manifest.yaml`（元数据）+ `entry` 指定的 Lua 入口脚本，
//! 可选 `resources/`（资源，宿主只给路径）与 `libs/`（受限 require 的纯 Lua 库）。
//! 契约与 Android 版一致（见 Xime 仓库 `plugin-core` 的 LuaPluginContract）：
//! 入口脚本 `return` 一个导出表，宿主按类型调用 `getCategories`/`getEmojis` 等函数；
//! 沙箱剥离 io/os/loadfile/dofile，插件只能访问注入的 `host` 白名单 API。

pub mod manifest;
pub mod host_api;
mod runtime;

pub mod manager;

pub use manifest::{NetworkDecl, PluginManifest, PluginType};
pub use runtime::NetworkPolicy;
pub use host_api::{
    ClipboardEntryInfo, ClipboardReadApi, HostApis, QuickSendItemInfo, QuickSendReadApi,
};
pub use runtime::{EmojiItem, EmojiLayout, PluginRuntime, RuntimeError, RuntimeResult, RuntimeCaps};

pub use manager::{PluginManager, PluginRecord, PluginRecordState};
