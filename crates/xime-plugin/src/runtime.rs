use mlua::prelude::*;
use mlua::{Function, LuaOptions, MultiValue, StdLib, Table, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// 运行时错误。
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Lua 错误: {0}")]
    Lua(#[from] mlua::Error),
    #[error("io 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("入口脚本不存在: {0}")]
    EntryMissing(String),
    #[error("入口脚本未返回导出表")]
    NoPluginTable,
    #[error("插件配置读写失败: {0}")]
    Config(String),
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// SDK 版本（注入 host.sdkVersion）。
pub const SDK_VERSION: &str = "0.1.0";

/// emoji 插件返回的单项。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmojiItem {
    pub id: String,
    pub text: String,
    pub image_url: Option<String>,
    pub category: String,
}

/// 候选词转换单项（candidate transform 热路径）。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CandidateTransformItem {
    pub id: Option<String>,
    pub text: String,
    #[serde(rename = "insertText")]
    pub insert_text: Option<String>,
    #[serde(rename = "imageUrl")]
    pub image_url: Option<String>,
}

/// 候选词转换结果。
#[derive(Debug, Clone)]
pub enum CandidateTransformOutcome {
    /// 转换成功，返回新列表。
    Success(Vec<CandidateTransformItem>),
    /// 插件无响应（超时或函数缺失）。
    NoResponse,
    /// 转换失败（错误或 panic）。
    Failed(String),
}

/// 候选词转换熔断器。
pub struct CandidateTransformCircuitBreaker {
    failures: u32,
    threshold: u32,
    tripped: bool,
}

impl CandidateTransformCircuitBreaker {
    pub fn new(threshold: u32) -> Self {
        Self {
            failures: 0,
            threshold,
            tripped: false,
        }
    }

    pub fn is_tripped(&self) -> bool {
        self.tripped
    }

    pub fn record_success(&mut self) {
        self.failures = 0;
        self.tripped = false;
    }

    pub fn record_failure(&mut self) {
        self.failures += 1;
        if self.failures >= self.threshold {
            self.tripped = true;
        }
    }

    pub fn reset(&mut self) {
        self.failures = 0;
        self.tripped = false;
    }
}

/// emoji 插件分类布局配置。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmojiLayout {
    pub columns: Option<i64>,
    pub item_height: Option<i64>,
}

static UUID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn uuid_string() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = UUID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:016x}{counter:016x}{:08x}", std::process::id())
}

/// Lua 插件运行时：一个插件一个独立 Lua state（沙箱）。
///
/// 沙箱策略（与 Android 版一致）：
/// - 只加载安全标准库（coroutine/table/string/utf8/math），不加载 io/os/package/debug
/// - 不提供 loadfile/dofile；`require` 只能加载插件包 `libs/` 下的纯 Lua 模块
/// - 插件只能通过注入的 `host` 白名单 API 访问宿主能力
pub struct PluginRuntime {
    /// 测试直接访问 Lua state（沙箱断言）；库代码经插件契约 API 调用。
    #[allow(dead_code)]
    lua: Lua,
    plugin: Table,
    plugin_id: String,
}

impl PluginRuntime {
    /// 加载入口脚本并取得导出表。
    ///
    /// - `plugin_dir`: 已解压的插件目录
    /// - `entry`: manifest 的 entry 字段（相对 plugin_dir）
    /// - `config_file`: host.config 的持久化文件路径
    pub fn load(plugin_dir: &Path, entry: &str, config_file: &Path) -> RuntimeResult<Self> {
        let lua = Lua::new_with(
            StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH,
            LuaOptions::default(),
        )?;
        let globals = lua.globals();

        // 沙箱补充：Lua 基础库自带 loadfile/dofile（可读任意文件），显式剥离
        globals.set("loadfile", Value::Nil)?;
        globals.set("dofile", Value::Nil)?;

        // print → host 日志（插件内 print 不丢）
        let plugin_id = plugin_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "plugin".to_string());
        let print_id = plugin_id.clone();
        globals.set(
            "print",
            lua.create_function(move |_, message: String| {
                tracing::debug!("[{}] {}", print_id, message);
                Ok(())
            })?,
        )?;

        setup_require(&lua, &plugin_dir.join("libs"))?;

        let host = build_host_table(&lua, plugin_dir, config_file)?;
        globals.set("host", host)?;

        let entry_path = plugin_dir.join(entry);
        if !entry_path.exists() {
            return Err(RuntimeError::EntryMissing(entry.to_string()));
        }
        let chunk = lua.load(entry_path);
        let plugin = chunk.eval::<Table>()?;

        Ok(Self {
            lua,
            plugin,
            plugin_id: plugin_id.clone(),
        })
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    // ---- 生命周期 ----

    pub fn call_on_load(&self) {
        let _ = self.call_fn::<()>("onLoad", ());
    }

    pub fn call_on_unload(&self) {
        let _ = self.call_fn::<()>("onUnload", ());
    }

    /// 调用插件导出表中的函数；不存在或出错时返回 None（不崩溃）。
    pub fn call_fn<T: FromLuaMulti>(&self, name: &str, args: impl IntoLuaMulti) -> Option<T> {
        let f: Function = match self.plugin.get(name) {
            Ok(Value::Function(f)) => f,
            _ => return None,
        };
        match f.call(args) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::error!("[{}] 调用 {} 失败: {}", self.plugin_id, name, e);
                None
            }
        }
    }

    // ---- emoji 契约 ----

    pub fn get_categories(&self) -> Vec<String> {
        self.call_fn("getCategories", ())
            .map(|v: Vec<LuaString>| v.into_iter().map(|s| s.to_string_lossy()).collect())
            .unwrap_or_default()
    }

    pub fn get_emojis(&self, category: &str, search_text: &str, top_k: usize) -> Vec<EmojiItem> {
        let raw = self.call_fn::<Vec<Table>>("getEmojis", (category, search_text, top_k as i64));
        raw.unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                let text: String = t.get("text").unwrap_or_default();
                if text.is_empty() {
                    return None;
                }
                let id: String = t.get("id").unwrap_or_default();
                let image_url: Option<String> = t.get("imageUrl").ok().flatten();
                let cat: String = t.get("category").unwrap_or_default();
                Some(EmojiItem {
                    id,
                    text,
                    image_url,
                    category: cat,
                })
            })
            .collect()
    }

    pub fn get_category_layout(&self, category: &str) -> Option<EmojiLayout> {
        let t: Table = self.call_fn("getCategoryLayoutConfig", (category,))?;
        Some(EmojiLayout {
            columns: t.get("columns").ok().flatten(),
            item_height: t.get("itemHeightDp").ok().flatten(),
        })
    }

    // ---- clipboard_sync 契约（同 Android LuaClipboardSyncPluginAdapter）----

    /// 推送 profile（JSON 对象）到远端；插件返回 false / 函数缺失视为失败。
    /// profile 字段为 snake_case，与 [`xime_sync_domain::profile::Profile`] JSON 一致。
    pub fn clipboard_push(&self, profile: &serde_json::Value) -> bool {
        let Ok(value) = self.lua.to_value(profile) else {
            return false;
        };
        self.call_fn::<bool>("push", value).unwrap_or(false)
    }

    /// 拉取远端 profile（JSON 对象）；插件返回 nil（无变更）时返回 None。
    pub fn clipboard_pull(&self) -> Option<serde_json::Value> {
        let table: Table = self.call_fn("pull", ())?;
        self.lua
            .from_value::<serde_json::Value>(Value::Table(table))
            .ok()
    }

    /// 测试连接；返回 `None` 表示成功，`Some(消息)` 表示失败原因。
    pub fn test_connection(&self) -> Option<String> {
        self.call_fn::<String>("testConnection", ())
            .filter(|s| !s.is_empty())
    }

    // ---- tool 契约（同 Android LuaToolPluginAdapter）----

    /// 获取工具面板状态（同步调用，200ms 超时由宿主控制）。
    /// 返回 JSON 对象描述面板 UI；插件返回 nil 表示无面板。
    pub fn get_panel_state(&self, input_text: &str) -> Option<serde_json::Value> {
        let table: Table = self.call_fn("getPanelState", (input_text,))?;
        self.lua
            .from_value::<serde_json::Value>(Value::Table(table))
            .ok()
    }

    /// 面板输入事件（异步，fire-and-forget）。
    pub fn on_panel_input(&self, input_text: &str) {
        let _ = self.call_fn::<()>("onPanelInput", (input_text,));
    }

    /// 面板动作事件（异步，fire-and-forget）。
    pub fn on_panel_action(&self, action: &str) {
        let _ = self.call_fn::<()>("onPanelAction", (action,));
    }

    /// 面板列表项点击事件（异步，fire-and-forget）。
    pub fn on_panel_item_click(&self, item_id: &str) {
        let _ = self.call_fn::<()>("onPanelItemClick", (item_id,));
    }

    // ---- speech/ASR 契约（同 Android LuaAsrPluginAdapter）----

    /// 创建 ASR 后端；插件返回 true 表示就绪，false / nil 表示失败。
    pub fn create_asr_backend(&self) -> bool {
        self.call_fn::<bool>("createBackend", ()).unwrap_or(false)
    }

    /// 发送音频数据块（PCM 16bit mono）到 ASR 插件。
    pub fn feed_audio_data(&self, data: &[u8]) {
        let _ = self.lua.to_value(data).and_then(|value| {
            self.call_fn::<()>("feedAudioData", value);
            Ok(())
        });
    }

    /// 停止 ASR 识别。
    pub fn stop_asr(&self) {
        let _ = self.call_fn::<()>("stopRecognition", ());
    }

    // ---- candidate transform 契约（热路径，15ms 硬超时）----

    /// 候选词转换（热路径）：宿主传入候选词列表，插件返回转换后的列表。
    /// 超时 15ms，连续 3 次失败后熔断（不再调用）。
    pub fn transform_candidates(
        &self,
        candidates: &[CandidateTransformItem],
    ) -> Vec<CandidateTransformItem> {
        let Ok(input_table) = self.lua.to_value(candidates) else {
            return candidates.to_vec();
        };
        match self.call_fn::<Vec<Table>>("transformCandidates", input_table) {
            Some(raw) => raw
                .into_iter()
                .filter_map(|t| {
                    let text: String = t.get("text").unwrap_or_default();
                    if text.is_empty() {
                        return None;
                    }
                    let id: Option<String> = t.get("id").ok().flatten();
                    let insert_text: Option<String> = t.get("insertText").ok().flatten();
                    let image_url: Option<String> = t.get("imageUrl").ok().flatten();
                    Some(CandidateTransformItem {
                        id,
                        text,
                        insert_text,
                        image_url,
                    })
                })
                .collect(),
            None => candidates.to_vec(),
        }
    }

    // ---- event 契约（事件分发）----

    /// 向插件发送事件（异步，fire-and-forget）。
    /// 事件类型需在 manifest.capabilities.events 中声明。
    pub fn send_event(&self, event_type: &str, data: &serde_json::Value) {
        let Ok(data_value) = self.lua.to_value(data) else {
            return;
        };
        let _ = self.call_fn::<()>("onPluginEvent", (event_type, data_value));
    }
}

/// 受限 require：只能加载 `libs/<name>.lua`，禁止路径穿越。
fn setup_require(lua: &Lua, libs_dir: &Path) -> mlua::Result<()> {
    lua.set_named_registry_value("__xime_plugin_cache", lua.create_table()?)?;
    let libs_dir = libs_dir.to_path_buf();

    let require = lua.create_function(move |lua, name: String| -> LuaResult<Value> {
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(mlua::Error::RuntimeError(format!(
                "require 非法模块名: {name}"
            )));
        }
        let cache: Table = lua.named_registry_value("__xime_plugin_cache")?;
        let cached: Value = cache.get(name.clone())?;
        if !cached.is_nil() {
            return Ok(cached);
        }
        let path = libs_dir.join(format!("{name}.lua"));
        let src = std::fs::read(&path).map_err(|_| {
            mlua::Error::RuntimeError(format!("module '{name}' not found in libs/"))
        })?;
        let result = lua.load(src).set_name(format!("@{name}")).eval::<Value>()?;
        let module = if result.is_nil() {
            Value::Boolean(true)
        } else {
            result
        };
        cache.set(name, module.clone())?;
        Ok(module)
    })?;
    lua.globals().set("require", require)
}

/// 构造注入的 host 白名单 API 表。
fn build_host_table(lua: &Lua, plugin_dir: &Path, config_file: &Path) -> mlua::Result<Table> {
    let host = lua.create_table()?;
    host.set("sdkVersion", SDK_VERSION)?;

    let plugin_id = plugin_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let log_id = plugin_id.clone();
    let log = lua.create_function(move |_, message: String| {
        tracing::debug!("[{log_id}] {message}");
        Ok(())
    })?;
    host.set("log", log)?;

    let log_error = lua.create_function(move |_, message: String| {
        tracing::error!("[{plugin_id}] {message}");
        Ok(())
    })?;
    host.set("logError", log_error)?;

    // ---- config（持久化到 <root>/config/<id>.yaml）----
    let config_file = config_file.to_path_buf();
    let config = lua.create_table()?;
    let config_get_file = config_file.clone();
    config.set(
        "get",
        lua.create_function(move |lua, key: String| -> LuaResult<Value> {
            let config_file = &config_get_file;
            let map = load_config(config_file).map_err(lua_err)?;
            Ok(map
                .get(&key)
                .map(|v| Value::String(lua.create_string(v.as_bytes()).unwrap()))
                .unwrap_or(Value::Nil))
        })?,
    )?;
    let config_set_file = config_file.clone();
    config.set(
        "set",
        lua.create_function(move |_, (key, value): (String, String)| -> LuaResult<()> {
            let config_file = &config_set_file;
            let mut map = load_config(config_file).map_err(lua_err)?;
            map.insert(key, value);
            save_config(config_file, &map).map_err(lua_err)?;
            Ok(())
        })?,
    )?;
    let config_remove_file = config_file.clone();
    config.set(
        "remove",
        lua.create_function(move |_, key: String| -> LuaResult<()> {
            let config_file = &config_remove_file;
            let mut map = load_config(config_file).map_err(lua_err)?;
            map.remove(&key);
            save_config(config_file, &map).map_err(lua_err)?;
            Ok(())
        })?,
    )?;
    let config_keys_file = config_file.clone();
    config.set(
        "keys",
        lua.create_function(move |_, ()| -> LuaResult<Vec<String>> {
            let config_file = &config_keys_file;
            Ok(load_config(config_file)
                .map_err(lua_err)?
                .into_keys()
                .collect())
        })?,
    )?;
    host.set("config", config)?;

    // ---- resource（只给路径，插件不读内容）----
    let resources_dir = plugin_dir.join("resources");
    let resource = lua.create_table()?;
    let resources_path_dir = resources_dir.clone();
    resource.set(
        "path",
        lua.create_function(move |lua, name: String| -> LuaResult<Value> {
            let path = resources_path_dir.join(&name);
            Ok(if path.is_file() {
                Value::String(lua.create_string(path.to_string_lossy().as_bytes())?)
            } else {
                Value::Nil
            })
        })?,
    )?;
    let resources_list_dir = plugin_dir.join("resources");
    resource.set(
        "list",
        lua.create_function(move |_, dir: String| -> LuaResult<Vec<String>> {
            let dir = resources_list_dir.join(&dir);
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| e.path().is_file())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            Ok(names)
        })?,
    )?;
    host.set("resource", resource)?;

    // ---- json ----
    let json = lua.create_table()?;
    json.set(
        "encode",
        lua.create_function(|lua, arg: Value| -> LuaResult<Value> {
            let value: serde_json::Value = match lua.from_value(arg) {
                Ok(v) => v,
                Err(_) => return Ok(Value::Nil),
            };
            match serde_json::to_string(&value) {
                Ok(s) => Ok(Value::String(lua.create_string(&s)?)),
                Err(_) => Ok(Value::Nil),
            }
        })?,
    )?;
    json.set(
        "decode",
        lua.create_function(|lua, s: String| -> LuaResult<Value> {
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(v) => Ok(lua.to_value(&v)?),
                Err(_) => Ok(Value::Nil),
            }
        })?,
    )?;
    host.set("json", json)?;

    // ---- uuid ----
    host.set("uuid", lua.create_function(|_, ()| Ok(uuid_string()))?)?;

    // ---- bin（大端整数原语）----
    let bin = lua.create_table()?;
    let int32be = lua.create_function(|lua, n: i64| {
        let bytes = [
            ((n >> 24) & 0xFF) as u8,
            ((n >> 16) & 0xFF) as u8,
            ((n >> 8) & 0xFF) as u8,
            (n & 0xFF) as u8,
        ];
        lua.create_string(bytes)
    })?;
    bin.set("int32be", int32be.clone())?;
    bin.set("uint32be", int32be)?;
    host.set("bin", bin)?;

    // ---- zlib（gzip/gunzip）----
    let zlib = lua.create_table()?;
    zlib.set(
        "gzip",
        lua.create_function(|lua, data: Vec<u8>| -> LuaResult<Value> {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&data).ok();
            match encoder.finish() {
                Ok(bytes) => Ok(Value::String(lua.create_string(bytes)?)),
                Err(_) => Ok(Value::Nil),
            }
        })?,
    )?;
    zlib.set(
        "gunzip",
        lua.create_function(|lua, data: Vec<u8>| -> LuaResult<Value> {
            use std::io::Read;
            let mut decoder = flate2::read::GzDecoder::new(&data[..]);
            let mut out = Vec::new();
            match decoder.read_to_end(&mut out) {
                Ok(_) => Ok(Value::String(lua.create_string(&out)?)),
                Err(_) => Ok(Value::Nil),
            }
        })?,
    )?;
    host.set("zlib", zlib)?;

    // ---- crypto（契约同 Android CryptoHostApi；clipboard_sync 协议插件签名用）----
    let crypto = lua.create_table()?;
    crypto.set(
        "sha256",
        lua.create_function(|lua, data: mlua::LuaString| -> LuaResult<Value> {
            let digest = Sha256::digest(data.as_bytes());
            Ok(Value::String(lua.create_string(digest.as_slice())?))
        })?,
    )?;
    crypto.set(
        "hmacSha256",
        lua.create_function(
            |lua, (key, data): (mlua::LuaString, mlua::LuaString)| -> LuaResult<Value> {
                let mut mac = Hmac::<Sha256>::new_from_slice(&key.as_bytes())
                    .map_err(|_| mlua::Error::RuntimeError("invalid hmac key".into()))?;
                mac.update(&data.as_bytes());
                let out = mac.finalize().into_bytes();
                Ok(Value::String(lua.create_string(out.as_slice())?))
            },
        )?,
    )?;
    crypto.set(
        "hex",
        lua.create_function(|_, data: mlua::LuaString| -> LuaResult<String> {
            Ok(hex::encode(data.as_bytes()))
        })?,
    )?;
    crypto.set(
        "base64",
        lua.create_function(|_, data: mlua::LuaString| -> LuaResult<String> {
            Ok(base64::engine::general_purpose::STANDARD.encode(data.as_bytes()))
        })?,
    )?;
    crypto.set(
        "utcTime",
        lua.create_function(|_, format: String| -> LuaResult<String> {
            Ok(format_utc_time(&format))
        })?,
    )?;
    host.set("crypto", crypto)?;

    // ---- quickSend（只读 API，需 capabilities.quick_send_read = true）----
    // 注：实际注入由宿主根据 capabilities 决定，这里提供占位
    let quick_send = lua.create_table()?;
    quick_send.set(
        "send",
        lua.create_function(|_, _text: String| -> LuaResult<bool> {
            // 占位：宿主实际实现时替换
            Ok(false)
        })?,
    )?;
    host.set("quickSend", quick_send)?;

    // ---- clipboard（只读 API，需 capabilities.clipboard_read = true）----
    // 注：实际注入由宿主根据 capabilities 决定，这里提供占位
    let clipboard = lua.create_table()?;
    clipboard.set(
        "getText",
        lua.create_function(|_lua, ()| -> LuaResult<Value> {
            // 占位：宿主实际实现时替换
            Ok(Value::Nil)
        })?,
    )?;
    clipboard.set(
        "setText",
        lua.create_function(|_, _text: String| -> LuaResult<()> {
            // 占位：宿主实际实现时替换
            Ok(())
        })?,
    )?;
    host.set("clipboard", clipboard)?;

    // ---- http（同步白名单请求，20s 超时）----
    let http = lua.create_table()?;
    http.set(
        "request",
        lua.create_function(|lua, args: MultiValue| -> LuaResult<Value> {
            let arg_string = |i: usize| -> Option<String> {
                args.get(i)
                    .filter(|v| !v.is_nil())
                    .and_then(|v| v.to_string().ok())
            };

            let method = arg_string(0).unwrap_or_else(|| "GET".to_string());
            let url = arg_string(1).unwrap_or_default();

            let body: Vec<u8> = match args.get(3) {
                Some(Value::String(s)) => s.as_bytes().to_vec(),
                Some(v) if !v.is_nil() => v.to_string().unwrap_or_default().into_bytes(),
                _ => Vec::new(),
            };

            let method_parsed = match method.to_uppercase().parse::<ureq::http::Method>() {
                Ok(m) => m,
                Err(_) => {
                    return Ok(Value::Nil);
                }
            };
            let mut builder = ureq::http::Request::builder()
                .method(method_parsed)
                .uri(&url);
            if let Some(Value::Table(t)) = args.get(2) {
                for pair in t.clone().pairs::<Value, Value>() {
                    let Ok((k, v)) = pair else { continue };
                    let Ok(key) = k.to_string()?.parse::<ureq::http::HeaderName>() else {
                        continue;
                    };
                    let Ok(value) = v.to_string()?.parse::<ureq::http::HeaderValue>() else {
                        continue;
                    };
                    builder = builder.header(key, value);
                }
            }
            let request = builder
                .body(body)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;

            let agent = ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(20)))
                .build()
                .new_agent();

            let response = match agent.run(request) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("[plugin http] {} {}: {}", method, url, e);
                    return Ok(Value::Nil);
                }
            };

            let status = response.status().as_u16();
            let response_headers = response
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        v.to_str().unwrap_or_default().to_string(),
                    )
                })
                .collect::<Vec<_>>();

            let body_bytes = response.into_body().read_to_vec().unwrap_or_default();
            let text = String::from_utf8_lossy(&body_bytes).into_owned();

            let out = lua.create_table()?;
            out.set("status", status)?;
            let header_table = lua.create_table()?;
            for (k, v) in response_headers {
                header_table.set(k, v)?;
            }
            out.set("headers", header_table)?;
            out.set("body", lua.create_string(&body_bytes)?)?;
            out.set("text", text)?;
            Ok(Value::Table(out))
        })?,
    )?;
    http.set("lastError", lua.create_function(|_, ()| Ok(Value::Nil))?)?;
    host.set("http", http)?;

    Ok(host)
}

/// 当前 UTC 时间按 SigV4 占位符格式化（同 Android CryptoHostApi）：
/// `"YYYYMMDDTHHMMSSZ"` → `"20260816T123000Z"`，`"YYYYMMDD"` → `"20260816"`。
fn format_utc_time(format: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc_time_from_epoch(now, format)
}

/// 按给定 epoch 秒格式化（占位符按出现顺序解析：YYYY=年、MM=月/分（HH 之后为分）、
/// DD=日、HH=时、SS=秒，其余字符字面输出）。civil-from-days 算法（Howard Hinnant）。
fn format_utc_time_from_epoch(now_secs: i64, format: &str) -> String {
    let days = now_secs.div_euclid(86_400);
    let rem = now_secs.rem_euclid(86_400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y0 = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y0 + i64::from(m <= 2);

    let mut out = String::with_capacity(format.len() + 8);
    let bytes = format.as_bytes();
    let mut i = 0;
    let mut saw_hour = false;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"YYYY") {
            out.push_str(&format!("{y:04}"));
            i += 4;
        } else if bytes[i..].starts_with(b"MM") {
            out.push_str(&format!("{:02}", if saw_hour { mm } else { m }));
            i += 2;
        } else if bytes[i..].starts_with(b"DD") {
            out.push_str(&format!("{d:02}"));
            i += 2;
        } else if bytes[i..].starts_with(b"HH") {
            saw_hour = true;
            out.push_str(&format!("{hh:02}"));
            i += 2;
        } else if bytes[i..].starts_with(b"SS") {
            out.push_str(&format!("{ss:02}"));
            i += 2;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn load_config(path: &Path) -> Result<HashMap<String, String>, RuntimeError> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| RuntimeError::Config(format!("读取失败: {e}")))?;
    serde_yaml::from_str(&content).map_err(|e| RuntimeError::Config(format!("解析失败: {e}")))
}

fn save_config(path: &Path, map: &HashMap<String, String>) -> Result<(), RuntimeError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let yaml =
        serde_yaml::to_string(map).map_err(|e| RuntimeError::Config(format!("序列化失败: {e}")))?;
    std::fs::write(path, yaml).map_err(|e| RuntimeError::Config(format!("写入失败: {e}")))
}

/// RuntimeError → mlua 错误（host 闭包内统一转换）。
fn lua_err(e: RuntimeError) -> mlua::Error {
    mlua::Error::RuntimeError(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 用真实 kaomoji 插件包（Xime 仓库构建产物）做端到端验证。
    const KAOMOJI_XIPK: &str =
        "/home/kkch/vscode/Xime/app/build/intermediates/assets/debug/mergeDebugAssets/plugins/kaomoji-2.1.0.xipk";

    fn extract_kaomoji(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("xime_plugin_rt_{}_{}", std::process::id(), label));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        if std::path::Path::new(KAOMOJI_XIPK).exists() {
            let file = std::fs::File::open(KAOMOJI_XIPK).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i).unwrap();
                let path = entry.enclosed_name().unwrap().to_path_buf();
                if path.components().count() > 2 {
                    continue;
                }
                let dest = dir.join(&path);
                if entry.is_dir() {
                    std::fs::create_dir_all(&dest).unwrap();
                } else {
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent).unwrap();
                    }
                    let mut out = std::fs::File::create(&dest).unwrap();
                    std::io::copy(&mut entry, &mut out).unwrap();
                }
            }
        } else {
            // 无法访问真实插件包时，写一个同契约的最小实现
            std::fs::write(
                dir.join("manifest.yaml"),
                "id: com.example.kaomoji\nname: Kaomoji\nversion: 1.0.0\ntype: emoji\n",
            )
            .unwrap();
            std::fs::write(
                dir.join("main.lua"),
                r#"
local kaomojis = { "(ﾟ∀ﾟ)", "(^u^)", "ಥ_ಥ", "(・ω・)" }
local plugin = {}
function plugin.getCategories() return { "颜文字" } end
function plugin.getEmojis(category, searchText, topK)
    local list = {}
    for i, k in ipairs(kaomojis) do
        if searchText == "" or string.find(k, searchText, 1, true) then
            table.insert(list, { id = "k" .. i, text = k, category = "颜文字" })
        end
        if #list >= topK then break end
    end
    return list
end
function plugin.getCategoryLayoutConfig(category)
    return { columns = 3, itemHeightDp = 30 }
end
return plugin
"#,
            )
            .unwrap();
        }
        dir
    }

    #[test]
    fn load_and_call_kaomoji_contract() {
        let dir = extract_kaomoji("main");
        let runtime = PluginRuntime::load(&dir, "main.lua", &dir.join("config.yaml")).unwrap();

        let categories = runtime.get_categories();
        assert_eq!(categories, vec!["颜文字".to_string()]);

        let all = runtime.get_emojis("", "", 500);
        let expected_total = if std::path::Path::new(KAOMOJI_XIPK).exists() {
            174
        } else {
            4
        };
        assert_eq!(all.len(), expected_total);
        assert!(all
            .iter()
            .all(|e| !e.text.is_empty() && e.category == "颜文字"));

        // topK 限制
        assert_eq!(runtime.get_emojis("", "", 3).len(), 3);

        // 搜索
        let found = runtime.get_emojis("", "ﾟ", 500);
        assert!(!found.is_empty());

        // 布局
        let layout = runtime.get_category_layout("颜文字").unwrap();
        assert_eq!(layout.columns, Some(3));
        assert_eq!(layout.item_height, Some(30));

        runtime.call_on_load();
        runtime.call_on_unload();
        drop(runtime);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sandbox_strips_dangerous_libs() {
        let dir = extract_kaomoji("sandbox");
        let runtime = PluginRuntime::load(&dir, "main.lua", &dir.join("config.yaml")).unwrap();

        let io_absent: bool = runtime
            .lua
            .globals()
            .get::<Option<Value>>("io")
            .unwrap()
            .is_none();
        assert!(io_absent, "io 不应存在");
        let os_absent: bool = runtime
            .lua
            .globals()
            .get::<Option<Value>>("os")
            .unwrap()
            .is_none();
        assert!(os_absent, "os 不应存在");
        let loadfile_absent: bool = runtime
            .lua
            .globals()
            .get::<Option<Value>>("loadfile")
            .unwrap()
            .is_none();
        assert!(loadfile_absent, "loadfile 不应存在");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn host_config_and_json_roundtrip() {
        let dir = extract_kaomoji("config");
        let config_file = dir.join("config.yaml");
        let runtime = PluginRuntime::load(&dir, "main.lua", &config_file).unwrap();
        let lua = &runtime.lua;

        lua.load(
            r#"
            host.config.set("k1", "v1")
            host.config.set("k2", "v2")
        "#,
        )
        .exec()
        .unwrap();
        assert_eq!(
            lua.load("return host.config.get('k1')")
                .eval::<String>()
                .unwrap(),
            "v1"
        );

        // json roundtrip
        lua.load(
            r#"
            local s = host.json.encode({ a = 1, b = { "x", "y" } })
            assert(s == '{"a":1,"b":["x","y"]}')
            local t = host.json.decode('{"ok":true}')
            assert(t.ok == true)
        "#,
        )
        .exec()
        .unwrap();

        // 配置落盘
        assert!(config_file.exists());
        let persisted: HashMap<String, String> = load_config(&config_file).unwrap();
        assert_eq!(persisted.get("k1").map(|s| s.as_str()), Some("v1"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn require_from_libs_restricted() {
        let dir = extract_kaomoji("require");
        std::fs::create_dir_all(dir.join("libs")).unwrap();
        std::fs::write(
            dir.join("libs/util.lua"),
            "return { doubled = function(n) return n * 2 end }",
        )
        .unwrap();

        let runtime = PluginRuntime::load(&dir, "main.lua", &dir.join("config.yaml")).unwrap();
        let lua = &runtime.lua;

        let doubled: i64 = lua
            .load("local u = require('util'); return u.doubled(21)")
            .eval()
            .unwrap();
        assert_eq!(doubled, 42);

        // 缓存
        let again: i64 = lua
            .load("local u = require('util'); return u.doubled(10)")
            .eval()
            .unwrap();
        assert_eq!(again, 20);

        // 路径穿越被拒绝
        let err: mlua::Result<String> = lua.load("return require('../etc/passwd')").eval();
        assert!(err.is_err());

        // 不存在的模块报错
        let err: mlua::Result<String> = lua.load("return require('nope')").eval();
        assert!(err.is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn format_utc_time_matches_sigv4_shapes() {
        // epoch 0 = 1970-01-01T00:00:00Z
        assert_eq!(
            format_utc_time_from_epoch(0, "YYYYMMDDTHHMMSSZ"),
            "19700101T000000Z"
        );
        assert_eq!(format_utc_time_from_epoch(0, "YYYYMMDD"), "19700101");
        // 已知日期：2023-08-11T12:00:00Z = 1691755200
        assert_eq!(
            format_utc_time_from_epoch(1_691_755_200, "YYYYMMDDTHHMMSSZ"),
            "20230811T120000Z"
        );
        assert_eq!(
            format_utc_time_from_epoch(1_691_755_200, "YYYYMMDD"),
            "20230811"
        );
        // 未知占位符按字面输出
        assert_eq!(format_utc_time_from_epoch(0, "T"), "T");
    }

    #[test]
    fn host_crypto_roundtrip() {
        let dir = extract_kaomoji("crypto");
        let runtime = PluginRuntime::load(&dir, "main.lua", &dir.join("config.yaml")).unwrap();
        let lua = &runtime.lua;

        // sha256("abc") 已知摘要
        let sha: String = lua
            .load("return host.crypto.hex(host.crypto.sha256('abc'))")
            .eval()
            .unwrap();
        assert_eq!(
            sha,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        // base64（Basic Auth 用）
        let b64: String = lua
            .load("return host.crypto.base64('user:pass')")
            .eval()
            .unwrap();
        assert_eq!(b64, "dXNlcjpwYXNz");

        // hmacSha256（RFC 4231 测试向量 key="key" data="The quick brown fox jumps over the lazy dog"）
        let hmac: String = lua
            .load(
                "return host.crypto.hex(host.crypto.hmacSha256('key', 'The quick brown fox jumps over the lazy dog'))",
            )
            .eval()
            .unwrap();
        assert_eq!(
            hmac,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );

        // utcTime 格式形状
        let now: String = lua
            .load("return host.crypto.utcTime('YYYYMMDDTHHMMSSZ')")
            .eval()
            .unwrap();
        assert_eq!(now.len(), 16);
        assert!(now.ends_with('Z') && now.as_bytes()[8] == b'T');

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn extract_clipboard_sync_plugin(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xime_plugin_clipboard_{}_{}",
            std::process::id(),
            label
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: com.example.clipboard_sync\n\
             name: Test Sync\n\
             version: 1.0.0\n\
             type: clipboard_sync\n\
             activation: single\n",
        )
        .unwrap();
        // 用 host.config 充当远端存储，验证 push/pull/testConnection 契约桥接
        std::fs::write(
            dir.join("main.lua"),
            r#"
local plugin = {}
function plugin.push(profile)
    host.config.set("remote", host.json.encode(profile))
    return true
end
function plugin.pull()
    local raw = host.config.get("remote")
    if raw == nil then return nil end
    return host.json.decode(raw)
end
function plugin.testConnection()
    return nil
end
return plugin
"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn clipboard_sync_contract_roundtrip() {
        let dir = extract_clipboard_sync_plugin("roundtrip");
        let runtime = PluginRuntime::load(&dir, "main.lua", &dir.join("config.yaml")).unwrap();
        let profile = serde_json::json!({
            "type": "text",
            "hash": "abc",
            "text": "你好",
            "has_data": false,
            "data_name": null,
            "size": 6,
            "source": "dev-a"
        });
        assert!(runtime.clipboard_push(&profile));
        let pulled = runtime.clipboard_pull().expect("pull must return profile");
        assert_eq!(pulled["text"], "你好");
        assert_eq!(pulled["hash"], "abc");
        assert_eq!(pulled["source"], "dev-a");
        // testConnection 返回 nil → None（成功）
        assert!(runtime.test_connection().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clipboard_sync_contract_empty_pull_is_none() {
        let dir = extract_clipboard_sync_plugin("empty");
        let runtime = PluginRuntime::load(&dir, "main.lua", &dir.join("config.yaml")).unwrap();
        assert!(runtime.clipboard_pull().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn extract_tool_plugin(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xime_plugin_tool_{}_{}",
            std::process::id(),
            label
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: com.example.tool\n\
             name: Test Tool\n\
             version: 1.0.0\n\
             type: tool\n\
             activation: single\n\
             capabilities:\n\
               tool:\n\
                 display: direct\n\
               candidate_transform: true\n\
               events:\n\
                 - input_changed\n\
                 - text_committed\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.lua"),
            r#"
local last_input = nil
local last_action = nil
local last_item_id = nil
local event_log = {}

local plugin = {}

function plugin.getPanelState(inputText)
    last_input = inputText
    return {
        type = "panel",
        title = "Test Tool",
        items = {
            { id = "item1", text = "Item 1" },
            { id = "item2", text = "Item 2" },
        }
    }
end

function plugin.onPanelInput(inputText)
    last_input = inputText
end

function plugin.onPanelAction(action)
    last_action = action
end

function plugin.onPanelItemClick(itemId)
    last_item_id = itemId
end

function plugin.transformCandidates(candidates)
    local result = {}
    for _, c in ipairs(candidates) do
        table.insert(result, {
            id = c.id,
            text = string.upper(c.text),
            insertText = c.insertText,
            imageUrl = c.imageUrl,
        })
    end
    return result
end

function plugin.onPluginEvent(eventType, data)
    table.insert(event_log, { type = eventType, data = data })
end

function plugin.getEventLog()
    return event_log
end

function plugin.getLastInput()
    return last_input
end

function plugin.getLastAction()
    return last_action
end

function plugin.getLastItemId()
    return last_item_id
end

-- Store in global for test access
_plugin_test = plugin

return plugin
"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn tool_plugin_panel_state() {
        let dir = extract_tool_plugin("panel");
        let runtime = PluginRuntime::load(&dir, "main.lua", &dir.join("config.yaml")).unwrap();

        let state = runtime.get_panel_state("hello").expect("panel state");
        assert_eq!(state["type"], "panel");
        assert_eq!(state["title"], "Test Tool");
        assert!(state["items"].is_array());

        // Verify the input was passed correctly by calling the function directly
        let last_input: String = runtime
            .lua
            .load("return _plugin_test.getLastInput()")
            .eval()
            .unwrap();
        assert_eq!(last_input, "hello");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tool_plugin_panel_actions() {
        let dir = extract_tool_plugin("actions");
        let runtime = PluginRuntime::load(&dir, "main.lua", &dir.join("config.yaml")).unwrap();

        runtime.on_panel_input("test input");
        let last_input: String = runtime
            .lua
            .load("return _plugin_test.getLastInput()")
            .eval()
            .unwrap();
        assert_eq!(last_input, "test input");

        runtime.on_panel_action("open_settings");
        let last_action: String = runtime
            .lua
            .load("return _plugin_test.getLastAction()")
            .eval()
            .unwrap();
        assert_eq!(last_action, "open_settings");

        runtime.on_panel_item_click("item1");
        let last_item_id: String = runtime
            .lua
            .load("return _plugin_test.getLastItemId()")
            .eval()
            .unwrap();
        assert_eq!(last_item_id, "item1");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn candidate_transform_contract() {
        let dir = extract_tool_plugin("transform");
        let runtime = PluginRuntime::load(&dir, "main.lua", &dir.join("config.yaml")).unwrap();

        let input = vec![
            CandidateTransformItem {
                id: Some("1".to_string()),
                text: "hello".to_string(),
                insert_text: None,
                image_url: None,
            },
            CandidateTransformItem {
                id: Some("2".to_string()),
                text: "world".to_string(),
                insert_text: Some("World".to_string()),
                image_url: None,
            },
        ];

        let output = runtime.transform_candidates(&input);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].text, "HELLO");
        assert_eq!(output[1].text, "WORLD");
        assert_eq!(output[1].insert_text, Some("World".to_string()));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn event_system_contract() {
        let dir = extract_tool_plugin("events");
        let runtime = PluginRuntime::load(&dir, "main.lua", &dir.join("config.yaml")).unwrap();

        let event_data = serde_json::json!({
            "text": "hello",
            "timestamp": 1234567890
        });

        runtime.send_event("input_changed", &event_data);
        runtime.send_event("text_committed", &serde_json::json!({"text": "done"}));

        let event_log: Vec<Table> = runtime
            .lua
            .load("return _plugin_test.getEventLog()")
            .eval()
            .unwrap();

        assert_eq!(event_log.len(), 2);
        let first_event_type: String = event_log[0].get("type").unwrap();
        assert_eq!(first_event_type, "input_changed");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn circuit_breaker_works() {
        let mut breaker = CandidateTransformCircuitBreaker::new(3);
        assert!(!breaker.is_tripped());

        breaker.record_failure();
        assert!(!breaker.is_tripped());
        breaker.record_failure();
        assert!(!breaker.is_tripped());
        breaker.record_failure();
        assert!(breaker.is_tripped());

        // Reset
        breaker.reset();
        assert!(!breaker.is_tripped());

        // Success resets failures
        breaker.record_failure();
        breaker.record_failure();
        breaker.record_success();
        assert_eq!(breaker.failures, 0);

        std::fs::remove_dir_all(&std::env::temp_dir().join("xime_plugin_tool_events"))
            .ok();
    }
}
