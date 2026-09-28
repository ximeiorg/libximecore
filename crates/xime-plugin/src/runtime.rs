//! QuickJS 插件运行时（契约对齐 xime 3.0 Android `JsScriptRuntime`）。
//!
//! 入口脚本为一个 IIFE，把导出对象挂到 `globalThis.plugin`（分组命名空间：
//! `emoji.listCategories`、`clipboardSync.push`、`backup.push`、`settings.schema`、
//! `transform.candidates`、`panel.state`、`events.onTextCommitted`…）。
//! 宿主注入 `host` 白名单 API（同步阻塞实现，JS `await` 普通值合法，Android
//! 插件的 await 写法无需改动）；插件导出方法返回 Promise 时由 Rust 侧阻塞落定。

use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use hmac::{Hmac, Mac};
use libquickjs_ng_sys as q;
use quickjs_rusty::serde::{from_js, to_js};
use quickjs_rusty::{Arguments, Context, ContextError, ExecutionError, OwnedJsValue};
use serde::de::DeserializeOwned;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::manifest::{NetworkDecl, PluginManifest};

/// 运行时错误。
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("JS 错误: {0}")]
    Js(String),
    #[error("io 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("入口脚本不存在: {0}")]
    EntryMissing(String),
    #[error("入口脚本未定义 plugin 导出对象")]
    NoPluginTable,
    #[error("插件配置读写失败: {0}")]
    Config(String),
}

impl From<ExecutionError> for RuntimeError {
    fn from(e: ExecutionError) -> Self {
        RuntimeError::Js(e.to_string())
    }
}

impl From<ContextError> for RuntimeError {
    fn from(e: ContextError) -> Self {
        RuntimeError::Js(e.to_string())
    }
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// SDK 版本（注入 host.sdkVersion，对齐 Android `JsPluginContract`）。
pub const SDK_VERSION: &str = "3.0.0";

/// 契约调用超时（对齐 Android `JsScriptRuntime` 常量）。
const TIMEOUT_TRANSFORM: Duration = Duration::from_millis(15);
const TIMEOUT_CALLBACK: Duration = Duration::from_secs(5);
const TIMEOUT_BUSINESS: Duration = Duration::from_secs(180);

/// 每插件 QuickJS 堆上限。
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
/// require 模块单文件上限（同 Android）。
const MODULE_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Promise 落定轮询上限（防插件返回永不落定的 Promise 时忙等）。
const MAX_SETTLE_SPINS: u32 = 1_000_000;

/// emoji 插件返回的单项。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmojiItem {
    pub id: String,
    pub text: String,
    pub image_url: Option<String>,
    pub category: String,
}

/// 云备份上传结果（契约同 Android `BackupResult`）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackupUploadResult {
    pub ok: bool,
    pub id: Option<String>,
    pub message: Option<String>,
}

impl BackupUploadResult {
    fn failed(message: &str) -> Self {
        Self {
            ok: false,
            id: None,
            message: Some(message.to_string()),
        }
    }
}

/// 远端备份条目（契约同 Android `RemoteBackupEntry`）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteBackupEntry {
    /// 条目 id（由插件定义，如 WebDAV 的远端绝对路径）。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 创建时间（Unix 秒；插件未返回为 0）。
    pub created_at: i64,
    /// 字节大小（插件未返回为 -1）。
    pub size: i64,
}

/// 插件配置表单字段（`settings.schema` 返回的 XimeUiNode 中
/// text/secret/button 子集的最佳努力映射；select/switch 等节点暂被跳过）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingField {
    pub key: String,
    pub label: String,
    /// "text" | "secret" | "button"（其余类型按 text 处理，未知类型跳过）。
    pub ftype: String,
    pub placeholder: Option<String>,
    pub help_text: Option<String>,
    pub required: bool,
    pub default_value: Option<String>,
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
///
/// Android 3.0 契约中布局来自 manifest `capabilities.emoji`（columns/itemHeightDp），
/// 不再提供运行时查询；此结构仅为兼容保留。
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

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 中断处理器：超过 deadline（epoch 毫秒，0 = 无限制）时中断 JS 执行。
unsafe extern "C" fn interrupt_handler(_rt: *mut q::JSRuntime, opaque: *mut c_void) -> c_int {
    let deadline = unsafe { &*(opaque as *const AtomicI64) };
    let d = deadline.load(Ordering::Relaxed);
    if d != 0 && now_millis() >= d {
        1
    } else {
        0
    }
}

/// 沙箱引导脚本：装配 host 命名空间、polyfill 与契约调用机制，最后屏蔽动态求值。
/// 前置条件：Rust 侧已注册全部 `__ximeNative_*` 回调。
const BOOTSTRAP_JS: &str = r#"
var __ximeOriginalFunction = Function;

function __ximeToBytes(v) {
  if (v == null) return null;
  if (v instanceof Uint8Array || Array.isArray(v)) return Array.from(v);
  if (typeof v === 'string') return Array.from(new TextEncoder().encode(v));
  return null;
}

// TextEncoder/TextDecoder polyfill（quickjs 不内置）
globalThis.TextEncoder = function () {
  this.encoding = 'utf-8';
  this.encode = function (s) { return Uint8Array.from(__ximeNative_utf8Encode(String(s))); };
};
globalThis.TextDecoder = function () {
  this.encoding = 'utf-8';
  this.decode = function (a) {
    var bytes = (a instanceof Uint8Array || Array.isArray(a)) ? Array.from(a) : [];
    return __ximeNative_utf8Decode(bytes) || '';
  };
};

globalThis.atob = function (s) {
  var bytes = __ximeNative_base64Decode(String(s));
  if (bytes == null) throw new Error('atob: 非法 base64');
  var out = '';
  for (var i = 0; i < bytes.length; i += 4096) {
    out += String.fromCharCode.apply(null, bytes.slice(i, i + 4096));
  }
  return out;
};
globalThis.btoa = function (s) { return __ximeNative_base64Encode(String(s)); };

globalThis.XimeError = function (code, message) {
  this.code = code;
  this.message = message;
  this.name = 'XimeError';
};
globalThis.XimeError.prototype = Object.create(Error.prototype);
globalThis.XimeError.prototype.constructor = globalThis.XimeError;
globalThis.definePlugin = function (plugin) { return plugin; };

var host = {
  sdkVersion: __ximeNative_sdkVersion(),
  log: function () { __ximeNative_log.apply(null, arguments); },
  logError: function () { __ximeNative_logError.apply(null, arguments); },
  uuid: __ximeNative_uuid,
  config: {
    get: __ximeNative_configGet,
    set: __ximeNative_configSet,
    remove: __ximeNative_configRemove,
    keys: __ximeNative_configKeys,
    getJson: function (key) {
      try {
        var raw = __ximeNative_configGet(String(key));
        return raw == null ? null : JSON.parse(raw);
      } catch (e) { return null; }
    },
  },
  resource: { path: __ximeNative_resourcePath, list: __ximeNative_resourceList },
  bin: {
    int32be: function (n) { return Uint8Array.from(__ximeNative_binInt32be(n | 0)); },
    uint32be: function (n) { return Uint8Array.from(__ximeNative_binInt32be(n | 0)); },
  },
  zlib: {
    gzip: function (data) {
      var r = __ximeNative_zlibGzip(__ximeToBytes(data));
      return r == null ? null : Uint8Array.from(r);
    },
    gunzip: function (data) {
      var r = __ximeNative_zlibGunzip(__ximeToBytes(data));
      return r == null ? null : Uint8Array.from(r);
    },
  },
  crypto: {
    sha256: function (data) { return Uint8Array.from(__ximeNative_cryptoSha256(__ximeToBytes(data))); },
    hmacSha256: function (key, data) {
      return Uint8Array.from(__ximeNative_cryptoHmacSha256(__ximeToBytes(key), __ximeToBytes(data)));
    },
    hmacSha1: function (key, data) {
      return Uint8Array.from(__ximeNative_cryptoHmacSha1(__ximeToBytes(key), __ximeToBytes(data)));
    },
    hex: function (data) { return __ximeNative_cryptoHex(__ximeToBytes(data)); },
    base64: function (data) { return __ximeNative_cryptoBase64(__ximeToBytes(data)); },
    utcTime: function (format) { return __ximeNative_cryptoUtcTime(String(format)); },
    epochSeconds: __ximeNative_cryptoEpochSeconds,
  },
  http: {
    request: async function (method, url, headers, body, timeoutMillis) {
      if (body instanceof Uint8Array) body = Array.from(body);
      var r = __ximeNative_httpRequest(method, url, headers, body, timeoutMillis);
      if (r != null && r.__ximeError) {
        throw new XimeError(r.__ximeError.code, r.__ximeError.message);
      }
      if (r == null) throw new XimeError('E_NETWORK', method + ' ' + url + ' 请求失败');
      if (r.body != null) r.body = Uint8Array.from(r.body);
      return r;
    },
  },
  quickSend: { list: function () { return []; } },
  clipboard: { get: function () { return null; } },
  capabilities: [],
};
host.has = function (name) { return name != null && host[name] !== undefined; };
globalThis.host = host;

// require：仅插件包内相对 .js 模块（解析/读取由宿主实现，带缓存）
var __ximeModuleCache = {};
globalThis.require = function (spec) {
  var resolved = __ximeNative_resolveModule(String(spec));
  if (resolved == null) throw new Error("Cannot find module '" + spec + "'");
  if (__ximeModuleCache[resolved] !== undefined) return __ximeModuleCache[resolved].exports;
  var source = __ximeNative_readModule(resolved);
  if (source == null) throw new Error("Cannot read module '" + resolved + "'");
  var module = { exports: {} };
  __ximeModuleCache[resolved] = module;
  var factory = __ximeOriginalFunction('exports', 'require', 'module', source);
  factory(module.exports, globalThis.require, module);
  return module.exports;
};

// console（转发宿主日志，同 Android bootstrap）
globalThis.console = {
  log: function () { __ximeNative_log.apply(null, arguments); },
  info: function () { __ximeNative_log.apply(null, arguments); },
  debug: function () { __ximeNative_log.apply(null, arguments); },
  trace: function () { __ximeNative_log.apply(null, arguments); },
  warn: function () { __ximeNative_log.apply(null, arguments); },
  error: function () { __ximeNative_logError.apply(null, arguments); },
};

// 契约调用机制：Rust 设置 __ximePath/__ximeArgs 后调用 __ximeInvoke()；
// 逐级判空 + typeof 检查实现"函数缺失 → undefined"降级，apply 保持 this 绑定。
globalThis.__ximeInvoke = function () {
  var p = globalThis.plugin;
  if (p == null || typeof p !== 'object') return undefined;
  var segs = String(globalThis.__ximePath || '').split('.');
  var cur = p;
  for (var i = 0; i < segs.length - 1; i++) {
    if (cur == null || typeof cur !== 'object') return undefined;
    cur = cur[segs[i]];
  }
  if (cur == null || typeof cur !== 'object') return undefined;
  var fn = cur[segs[segs.length - 1]];
  if (typeof fn !== 'function') return undefined;
  return fn.apply(cur, globalThis.__ximeArgs || []);
};

// Promise 落定槽：Rust 侧驱动 pending job 并轮询状态（1=fulfilled 2=rejected）
globalThis.__ximeSettle = function (value) {
  globalThis.__ximeSettleState = 0;
  globalThis.__ximeSettleValue = undefined;
  Promise.resolve(value).then(
    function (v) { globalThis.__ximeSettleState = 1; globalThis.__ximeSettleValue = v; },
    function (e) {
      globalThis.__ximeSettleState = 2;
      globalThis.__ximeSettleValue = (e && e.message) ? e.message : String(e);
    }
  );
};

// 沙箱屏蔽动态求值（require 已捕获原始 Function）
Object.defineProperty(globalThis, 'eval', { value: undefined, writable: false, configurable: false });
Object.defineProperty(globalThis, 'Function', { value: undefined, writable: false, configurable: false });
"#;

/// JS 插件运行时：一个插件一个独立 QuickJS Context（沙箱）。
///
/// 沙箱策略（与 Android 版一致）：
/// - QuickJS 语言层无文件/进程 API；`eval`/`Function` 被屏蔽，`require` 只能
///   加载插件包内相对 `.js` 模块（≤2MB，宿主解析路径，拒绝穿越）
/// - 插件只能通过注入的 `host` 白名单 API 访问宿主能力
/// - 契约调用带硬超时（中断处理器），超时后运行时熔断（后续调用全部降级）
///
/// `Context` 非 `Send`：运行时必须在创建线程内使用（daemon 侧 PluginHost 归
/// wayland 事件循环线程、clipboard_sync 桥归专用线程，均满足）。
pub struct PluginRuntime {
    context: Context,
    plugin_id: String,
    /// 中断处理器 deadline（epoch 毫秒；0 = 不限制）。Box 保证地址稳定。
    deadline: Box<AtomicI64>,
    /// 超时熔断标志：置位后所有契约调用直接降级。
    poisoned: AtomicBool,
    /// manifest.capabilities.events：下行事件订阅（send_event 门禁）。
    subscribed_events: Vec<String>,
}

impl std::fmt::Debug for PluginRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRuntime")
            .field("plugin_id", &self.plugin_id)
            .field("poisoned", &self.poisoned.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl PluginRuntime {
    /// 加载入口脚本并校验 `globalThis.plugin` 导出对象。
    ///
    /// - `plugin_dir`: 已解压的插件目录
    /// - `entry`: manifest 的 entry 字段（相对 plugin_dir，默认 main.js）
    /// - `config_file`: host.config 的持久化文件路径
    pub fn load(plugin_dir: &Path, entry: &str, config_file: &Path) -> RuntimeResult<Self> {
        let plugin_id = plugin_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "plugin".to_string());

        let context = Context::builder().memory_limit(MEMORY_LIMIT).build()?;

        // manifest 读取失败按最严策略（无域名授权、无事件订阅）。
        let subscribed_events = match PluginManifest::from_dir(plugin_dir) {
            Ok(m) => {
                install_native_api(&context, plugin_dir, config_file, &m.network)?;
                m.capabilities.events
            }
            Err(e) => {
                tracing::warn!("[{plugin_id}] 读取 manifest 失败，按最严策略处理: {e}");
                install_native_api(&context, plugin_dir, config_file, &NetworkDecl::default())?;
                Vec::new()
            }
        };
        context.update_stack_top();
        context.eval(BOOTSTRAP_JS, false)?;

        if !entry_is_safe(entry) || !plugin_dir.join(entry).is_file() {
            return Err(RuntimeError::EntryMissing(entry.to_string()));
        }
        let source = std::fs::read_to_string(plugin_dir.join(entry))?;
        context.eval(&source, false)?;

        let plugin_ok = context
            .global()?
            .property("plugin")?
            .map(|v| v.is_object() && !v.is_undefined())
            .unwrap_or(false);
        if !plugin_ok {
            return Err(RuntimeError::NoPluginTable);
        }

        let runtime = Self {
            context,
            plugin_id,
            deadline: Box::new(AtomicI64::new(0)),
            poisoned: AtomicBool::new(false),
            subscribed_events,
        };
        // 字段声明顺序保证 context 先于 deadline 析构（handler 指向 deadline）。
        runtime.context.set_interrupt_handler(
            Some(interrupt_handler),
            &*runtime.deadline as *const AtomicI64 as *mut c_void,
        );
        Ok(runtime)
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    // ---- 契约调用机制 ----

    fn set_deadline(&self, timeout: Duration) {
        self.deadline
            .store(now_millis() + timeout.as_millis() as i64, Ordering::Relaxed);
    }

    fn deadline_expired(&self) -> bool {
        let d = self.deadline.load(Ordering::Relaxed);
        d != 0 && now_millis() >= d
    }

    /// 调用插件导出方法（`path` 为 `globalThis.plugin` 下的点路径），返回原始值。
    /// 函数缺失/插件报错/超时一律返回 None 并记日志（不向上传播）。
    fn call_slot_value(
        &self,
        timeout: Duration,
        path: &str,
        args: Vec<serde_json::Value>,
        prep: Option<&str>,
    ) -> Option<OwnedJsValue> {
        if self.poisoned.load(Ordering::Relaxed) {
            return None;
        }
        let ptr = unsafe { self.context.context_raw() };
        self.set_deadline(timeout);
        self.context.update_stack_top();
        let result = (|| -> Result<Option<OwnedJsValue>, ExecutionError> {
            self.context.set_global("__ximePath", path.to_string())?;
            let args_value =
                to_js(ptr, &args).map_err(|e| ExecutionError::Internal(e.to_string()))?;
            self.context.set_global("__ximeArgs", args_value)?;
            if let Some(prep_src) = prep {
                self.context.eval(prep_src, false)?;
            }
            let value = self.context.eval("__ximeInvoke()", false)?;
            self.settle(value)
        })();
        let expired = self.deadline_expired();
        self.deadline.store(0, Ordering::Relaxed);
        match result {
            Ok(Some(v)) if !v.is_undefined() && !v.is_null() => Some(v),
            Ok(_) => None,
            Err(e) => {
                if expired {
                    self.poisoned.store(true, Ordering::Relaxed);
                    tracing::error!("[{}] 调用 {path} 超时，运行时熔断", self.plugin_id);
                } else {
                    tracing::error!("[{}] 调用 {path} 失败: {e}", self.plugin_id);
                }
                None
            }
        }
    }

    /// [`Self::call_slot_value`] 的类型化版本（serde 反序列化返回值）。
    fn call_slot<T: DeserializeOwned>(
        &self,
        timeout: Duration,
        path: &str,
        args: Vec<serde_json::Value>,
        prep: Option<&str>,
    ) -> Option<T> {
        let value = self.call_slot_value(timeout, path, args, prep)?;
        match from_js(unsafe { self.context.context_raw() }, &value) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::error!("[{}] {path} 返回值解析失败: {e}", self.plugin_id);
                None
            }
        }
    }

    /// 阻塞落定 Promise（对齐 Android callAsync 语义）：所有返回值统一走
    /// `__ximeSettle` 槽（Promise.resolve 对普通值立即生效），Rust 侧驱动
    /// pending job 并轮询状态（1=fulfilled 2=rejected）。
    fn settle(&self, value: OwnedJsValue) -> Result<Option<OwnedJsValue>, ExecutionError> {
        let global = self.context.global()?;
        let setter = global
            .property("__ximeSettle")?
            .filter(|v| v.is_function())
            .ok_or_else(|| ExecutionError::Internal("__ximeSettle 缺失".to_string()))?
            .try_into_function()?;
        setter.call(vec![value])?;
        let mut spins = 0u32;
        loop {
            self.context.execute_pending_job()?;
            let state = global
                .property("__ximeSettleState")?
                .filter(|v| !v.is_undefined());
            let state = match state {
                Some(v) => v.to_int().unwrap_or(0),
                None => 0,
            };
            match state {
                1 => return global.property("__ximeSettleValue"),
                2 => {
                    let msg = global
                        .property("__ximeSettleValue")?
                        .and_then(|v| v.js_to_string().ok())
                        .unwrap_or_else(|| "promise rejected".to_string());
                    return Err(ExecutionError::Internal(msg));
                }
                _ => {}
            }
            spins += 1;
            if spins > MAX_SETTLE_SPINS {
                return Err(ExecutionError::Internal(
                    "promise 未在预算内落定".to_string(),
                ));
            }
        }
    }

    /// 测试/调试辅助：按点路径调用插件导出方法，JSON 入参/出参。
    pub fn call_plugin_fn(
        &self,
        path: &str,
        args: &[serde_json::Value],
    ) -> Option<serde_json::Value> {
        self.call_slot(TIMEOUT_BUSINESS, path, args.to_vec(), None)
    }

    // ---- 生命周期 ----

    pub fn call_on_load(&self) {
        let _ = self.call_slot_value(TIMEOUT_BUSINESS, "onLoad", vec![], None);
    }

    pub fn call_on_unload(&self) {
        let _ = self.call_slot_value(TIMEOUT_BUSINESS, "onUnload", vec![], None);
    }

    // ---- emoji 契约 ----

    pub fn get_categories(&self) -> Vec<String> {
        self.call_slot(TIMEOUT_BUSINESS, "emoji.listCategories", vec![], None)
            .unwrap_or_default()
    }

    pub fn get_emojis(&self, category: &str, search_text: &str, top_k: usize) -> Vec<EmojiItem> {
        let arg = serde_json::json!({
            "category": category,
            "keyword": search_text,
            "topK": top_k,
        });
        let Some(value) = self.call_slot_value(TIMEOUT_BUSINESS, "emoji.query", vec![arg], None)
        else {
            return Vec::new();
        };
        let Ok(items) =
            from_js::<Vec<serde_json::Value>>(unsafe { self.context.context_raw() }, &value)
        else {
            return Vec::new();
        };
        items
            .into_iter()
            .filter_map(|item| {
                let text = item.get("text").and_then(|t| t.as_str())?.to_string();
                if text.is_empty() {
                    return None;
                }
                Some(EmojiItem {
                    id: item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    text,
                    image_url: item
                        .get("imageUrl")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    category: item
                        .get("category")
                        .and_then(|v| v.as_str())
                        .unwrap_or(category)
                        .to_string(),
                })
            })
            .collect()
    }

    /// Android 3.0 契约中布局来自 manifest（capabilities.emoji），运行时不再查询；
    /// 保留 API 兼容旧调用方，恒返回 None。
    pub fn get_category_layout(&self, _category: &str) -> Option<EmojiLayout> {
        None
    }

    // ---- clipboard_sync 契约（同 Android JsClipboardSyncPluginAdapter）----

    /// 推送 profile（宿主 snake_case JSON）到远端；插件收到 SDK camelCase 形态。
    /// 插件返回 false / 函数缺失 / 报错视为失败。
    pub fn clipboard_push(&self, profile: &serde_json::Value) -> bool {
        let arg = clipboard_profile_to_sdk(profile);
        self.call_slot::<serde_json::Value>(TIMEOUT_BUSINESS, "clipboardSync.push", vec![arg], None)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// 拉取远端 profile；插件返回 null/undefined（无变更）时返回 None。
    /// SDK camelCase → 宿主 snake_case；hash 缺失时按 text 补算 sha256
    /// （同 Android adapter；附件字节桌面宿主尚未启用，仅文本路径）。
    pub fn clipboard_pull(&self) -> Option<serde_json::Value> {
        let mut profile: serde_json::Value =
            self.call_slot(TIMEOUT_BUSINESS, "clipboardSync.pull", vec![], None)?;
        if let Some(obj) = profile.as_object_mut() {
            if let Some(v) = obj.remove("hasData") {
                obj.insert("has_data".to_string(), v);
            }
            if let Some(v) = obj.remove("dataName") {
                obj.insert("data_name".to_string(), v);
            }
        }
        let hash_missing = profile
            .get("hash")
            .and_then(|h| h.as_str())
            .is_none_or(str::is_empty);
        if hash_missing {
            let text = profile
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            if !text.is_empty() {
                let hash = hex::encode(Sha256::digest(text.as_bytes()));
                profile
                    .as_object_mut()?
                    .insert("hash".to_string(), serde_json::Value::String(hash));
            }
        }
        Some(profile)
    }

    /// 测试连接；返回 `None` 表示成功，`Some(消息)` 表示失败原因。
    /// 依次尝试 clipboardSync.test / backup.test（插件按类型实现其一）。
    pub fn test_connection(&self) -> Option<String> {
        for path in ["clipboardSync.test", "backup.test"] {
            if let Some(value) = self.call_slot_value(TIMEOUT_BUSINESS, path, vec![], None) {
                let message = value.js_to_string().unwrap_or_default();
                return if message.is_empty() {
                    None
                } else {
                    Some(message)
                };
            }
        }
        None
    }

    // ---- backup 契约（同 Android JsBackupPluginAdapter）----
    // 宿主打包/恢复，插件只承载传输协议（WebDAV/S3/自建 HTTP）。

    /// 上传备份包。插件返回 bool 或 {ok, id, message} 两种形态。
    pub fn push_backup(&self, name: &str, archive: &[u8]) -> BackupUploadResult {
        let arg = serde_json::json!({ "name": name, "archive": archive });
        // archive 以 JSON 数组过桥，调用前在 JS 侧转为 Uint8Array（契约形状）。
        let prep =
            "globalThis.__ximeArgs[0].archive = new Uint8Array(globalThis.__ximeArgs[0].archive);";
        let Some(value) =
            self.call_slot_value(TIMEOUT_BUSINESS, "backup.push", vec![arg], Some(prep))
        else {
            return BackupUploadResult::failed("backup.push 调用失败");
        };
        if let Ok(ok) = bool::try_from(value.clone()) {
            return BackupUploadResult {
                ok,
                id: None,
                message: None,
            };
        }
        match from_js::<serde_json::Value>(unsafe { self.context.context_raw() }, &value) {
            Ok(v) => BackupUploadResult {
                ok: v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false),
                id: v.get("id").and_then(|v| v.as_str()).map(str::to_owned),
                message: v.get("message").and_then(|v| v.as_str()).map(str::to_owned),
            },
            Err(_) => BackupUploadResult::failed("backup.push 返回类型无效"),
        }
    }

    /// 下载备份包（id = 插件 list() 返回的条目 id）；null/失败返回 None。
    pub fn pull_backup(&self, id: &str) -> Option<Vec<u8>> {
        let value = self.call_slot_value(
            TIMEOUT_BUSINESS,
            "backup.pull",
            vec![serde_json::json!(id)],
            None,
        )?;
        self.value_to_bytes(value)
    }

    /// 列出远端备份条目；失败返回 None（与 Android 语义一致，区别于空列表）。
    pub fn list_backups(&self) -> Option<Vec<RemoteBackupEntry>> {
        let value = self.call_slot_value(TIMEOUT_BUSINESS, "backup.list", vec![], None)?;
        let raw = from_js::<Vec<serde_json::Value>>(unsafe { self.context.context_raw() }, &value)
            .ok()?;
        let mut out = Vec::new();
        for item in raw {
            let id = match item.get("id").and_then(|v| v.as_str()) {
                Some(id) if !id.is_empty() => id.to_string(),
                _ => continue,
            };
            out.push(RemoteBackupEntry {
                name: item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&id)
                    .to_string(),
                id,
                created_at: item.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0),
                size: item.get("size").and_then(|v| v.as_i64()).unwrap_or(-1),
            });
        }
        Some(out)
    }

    /// 删除远端备份条目。
    pub fn delete_backup(&self, id: &str) -> bool {
        self.call_slot::<bool>(
            TIMEOUT_BUSINESS,
            "backup.remove",
            vec![serde_json::json!(id)],
            None,
        )
        .unwrap_or(false)
    }

    /// 配置表单 schema（XimeUiNode 的 text/secret/button 子集映射）。
    /// select/multi_select/switch 降级为文本输入（桌面表单子集）；section/
    /// divider/metric 等展示节点无 key 被过滤；≤64 节点。
    pub fn get_settings_schema(&self) -> Vec<SettingField> {
        let raw = self
            .call_slot::<Vec<serde_json::Value>>(TIMEOUT_BUSINESS, "settings.schema", vec![], None)
            .unwrap_or_default();
        raw.into_iter()
            .take(64)
            .filter_map(|node| {
                let key = node.get("key").and_then(|v| v.as_str())?.to_string();
                let node_type = node.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                let ftype = match node_type {
                    "secret" => "secret",
                    "button" | "action" => "button",
                    "text" | "input" | "textarea" | "number" | "metric" => "text",
                    "select" | "multi_select" | "switch" => {
                        tracing::debug!(
                            "[{}] settings.schema 节点 {key} 类型 {node_type} 降级为文本输入",
                            self.plugin_id
                        );
                        "text"
                    }
                    other => {
                        tracing::debug!(
                            "[{}] settings.schema 节点 {key} 类型 {other} 不支持，跳过",
                            self.plugin_id
                        );
                        return None;
                    }
                };
                Some(SettingField {
                    label: node
                        .get("label")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&key)
                        .to_string(),
                    key,
                    ftype: ftype.to_string(),
                    placeholder: node
                        .get("placeholder")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    help_text: node
                        .get("helpText")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    required: node
                        .get("required")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    default_value: node
                        .get("defaultValue")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                })
            })
            .collect()
    }

    // ---- tool 契约（同 Android JsToolPluginAdapter）----

    /// 获取工具面板状态；插件返回 null/undefined 表示无面板。
    pub fn get_panel_state(&self, input_text: &str) -> Option<serde_json::Value> {
        self.call_slot(
            TIMEOUT_BUSINESS,
            "panel.state",
            vec![serde_json::json!(input_text)],
            None,
        )
    }

    /// 面板输入事件（fire-and-forget，5s 超时）。
    pub fn on_panel_input(&self, input_text: &str) {
        let _ = self.call_slot_value(
            TIMEOUT_CALLBACK,
            "panel.onInput",
            vec![serde_json::json!(input_text)],
            None,
        );
    }

    /// 面板动作事件（fire-and-forget，5s 超时）。
    pub fn on_panel_action(&self, action: &str) {
        let _ = self.call_slot_value(
            TIMEOUT_CALLBACK,
            "panel.onAction",
            vec![serde_json::json!(action)],
            None,
        );
    }

    /// 面板列表项点击事件（fire-and-forget，5s 超时）。
    pub fn on_panel_item_click(&self, item_id: &str) {
        let _ = self.call_slot_value(
            TIMEOUT_CALLBACK,
            "panel.onItemClick",
            vec![serde_json::json!(item_id)],
            None,
        );
    }

    // ---- speech/ASR 契约（同 Android JsAsrPluginAdapter，best-effort）----

    /// 准备 ASR 后端（调用 plugin.speech.configure）；缺失/失败返回 false。
    pub fn create_asr_backend(&self) -> bool {
        self.call_slot_value(TIMEOUT_BUSINESS, "speech.configure", vec![], None)
            .is_some()
    }

    /// 发送音频数据块（PCM 16bit mono）到 ASR 插件。
    pub fn feed_audio_data(&self, data: &[u8]) {
        let prep = "globalThis.__ximeArgs[0] = new Uint8Array(globalThis.__ximeArgs[0]);";
        let _ = self.call_slot_value(
            TIMEOUT_CALLBACK,
            "speech.feed",
            vec![serde_json::to_value(data).unwrap_or_default()],
            Some(prep),
        );
    }

    /// 停止 ASR 识别。
    pub fn stop_asr(&self) {
        let _ = self.call_slot_value(TIMEOUT_CALLBACK, "speech.stop", vec![], None);
    }

    // ---- candidate transform 契约（热路径，15ms 硬超时）----

    /// 候选词转换（热路径）：构造 Android 契约请求
    /// `{inputText, preedit, asciiMode, candidates}`，插件返回
    /// `{candidates: [{engineIndex} | {text, comment?}]}` 或 null（不干预）。
    /// 超时/缺失/报错一律原样返回输入。
    pub fn transform_candidates(
        &self,
        input_text: &str,
        preedit: &str,
        ascii_mode: bool,
        candidates: &[CandidateTransformItem],
    ) -> Vec<CandidateTransformItem> {
        let request = serde_json::json!({
            "inputText": input_text,
            "preedit": preedit,
            "asciiMode": ascii_mode,
            "candidates": candidates,
        });
        let Some(value) = self.call_slot_value(
            TIMEOUT_TRANSFORM,
            "transform.candidates",
            vec![request],
            None,
        ) else {
            return candidates.to_vec();
        };
        let Ok(response) =
            from_js::<serde_json::Value>(unsafe { self.context.context_raw() }, &value)
        else {
            return candidates.to_vec();
        };
        let Some(items) = response.get("candidates").and_then(|v| v.as_array()) else {
            return candidates.to_vec();
        };
        let mut out = candidates.to_vec();
        for (i, item) in items.iter().enumerate().take(out.len()) {
            if let Some(idx) = item.get("engineIndex").and_then(|v| v.as_u64()) {
                if (idx as usize) < candidates.len() {
                    out[i] = candidates[idx as usize].clone();
                }
                continue;
            }
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                if text.is_empty() {
                    continue;
                }
                out[i] = CandidateTransformItem {
                    id: item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                        .or_else(|| candidates[i].id.clone()),
                    text: text.to_string(),
                    insert_text: item
                        .get("insertText")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    image_url: item
                        .get("imageUrl")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                };
            }
        }
        out
    }

    // ---- event 契约（事件分发）----

    /// 向插件发送下行事件（fire-and-forget，5s 超时）。
    /// 事件类型必须在 manifest.capabilities.events 中声明（同 Android
    /// `initEvents` 订阅门禁）；slot 命名 snake_case → onPascalCase
    /// （同 Android `eventSlotName`）：text_committed → plugin.events.onTextCommitted(payload)。
    pub fn send_event(&self, event_type: &str, data: &serde_json::Value) {
        if !self.subscribed_events.iter().any(|e| e == event_type) {
            return;
        }
        let path = format!("events.{}", event_slot_name(event_type));
        let _ = self.call_slot_value(TIMEOUT_CALLBACK, &path, vec![data.clone()], None);
    }

    // ---- 私有辅助 ----

    /// 经 bootstrap `__ximeToBytes` 把 Uint8Array/Array/字符串统一转字节数组。
    fn value_to_bytes(&self, value: OwnedJsValue) -> Option<Vec<u8>> {
        let global = self.context.global().ok()?;
        let converter = global
            .property("__ximeToBytes")
            .ok()?
            .filter(|v| v.is_function())?;
        let converted = converter.try_into_function().ok()?.call(vec![value]).ok()?;
        from_js(unsafe { self.context.context_raw() }, &converted).ok()
    }
}

/// 入口路径安全检查：禁空、绝对路径、反斜杠与 `..` 穿越。
fn entry_is_safe(entry: &str) -> bool {
    !entry.is_empty()
        && !entry.contains('\\')
        && !entry.starts_with('/')
        && Path::new(entry)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// 事件类型 → slot 名（同 Android `JsPluginContract.eventSlotName`）。
fn event_slot_name(event_type: &str) -> String {
    let mut out = String::from("on");
    for segment in event_type.split('_') {
        let mut chars = segment.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// 注册全部 `__ximeNative_*` 原生回调（bootstrap 装配前的裸 API）。
fn install_native_api(
    context: &Context,
    plugin_dir: &Path,
    config_file: &Path,
    network: &NetworkDecl,
) -> Result<(), RuntimeError> {
    let ptr = unsafe { context.context_raw() };
    let config_file = config_file.to_path_buf();
    let resources_dir = plugin_dir.join("resources");
    let plugin_dir = plugin_dir.to_path_buf();

    context.add_callback("__ximeNative_sdkVersion", || SDK_VERSION.to_string())?;

    let log_id = plugin_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let err_id = log_id.clone();
    context.add_callback("__ximeNative_log", move |args: Arguments| {
        tracing::debug!("[{log_id}] {}", join_args(args));
    })?;
    context.add_callback("__ximeNative_logError", move |args: Arguments| {
        tracing::error!("[{err_id}] {}", join_args(args));
    })?;
    context.add_callback("__ximeNative_uuid", uuid_string)?;

    // ---- config（持久化到 <root>/config/<id>.yaml）----
    let file = config_file.clone();
    context.add_callback(
        "__ximeNative_configGet",
        move |key: String| -> Option<String> { load_config(&file).ok()?.remove(&key) },
    )?;
    let file = config_file.clone();
    context.add_callback(
        "__ximeNative_configSet",
        move |key: String, value: String| -> Result<bool, String> {
            let mut map = load_config(&file).map_err(|e| e.to_string())?;
            map.insert(key, value);
            save_config(&file, &map).map_err(|e| e.to_string())?;
            Ok(true)
        },
    )?;
    let file = config_file.clone();
    context.add_callback(
        "__ximeNative_configRemove",
        move |key: String| -> Result<bool, String> {
            let mut map = load_config(&file).map_err(|e| e.to_string())?;
            map.remove(&key);
            save_config(&file, &map).map_err(|e| e.to_string())?;
            Ok(true)
        },
    )?;
    let file = config_file.clone();
    context.add_callback("__ximeNative_configKeys", move || -> Vec<String> {
        load_config(&file)
            .map(|m| m.into_keys().collect())
            .unwrap_or_default()
    })?;

    // 网络白名单策略（同 Android NetworkPolicy 三重门的桌面等价物）：
    // ① manifest.network.hosts 声明域名；② allowCustomHosts 时，插件配置值中
    // 出现过的服务器域名（用户配置即授权）。未命中 → fail-closed E_DENIED。
    let network = network.clone();
    let policy_config = config_file.clone();

    // ---- resource（只给路径，插件不读内容）----
    let dir = resources_dir.clone();
    context.add_callback(
        "__ximeNative_resourcePath",
        move |name: String| -> Option<String> {
            if name.split(['/', '\\']).any(|seg| seg == "..") {
                return None;
            }
            let path = dir.join(name);
            path.is_file().then(|| path.to_string_lossy().to_string())
        },
    )?;
    let dir = resources_dir;
    context.add_callback(
        "__ximeNative_resourceList",
        move |sub: String| -> Vec<String> {
            let target = dir.join(sub);
            let mut names: Vec<String> = std::fs::read_dir(&target)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| e.path().is_file())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        },
    )?;

    // ---- bin（大端整数原语）----
    context.add_callback("__ximeNative_binInt32be", |n: i32| -> Vec<u8> {
        vec![
            ((n >> 24) & 0xFF) as u8,
            ((n >> 16) & 0xFF) as u8,
            ((n >> 8) & 0xFF) as u8,
            (n & 0xFF) as u8,
        ]
    })?;

    // ---- zlib（gzip/gunzip；入参为 JS 侧转好的字节数组）----
    context.add_callback(
        "__ximeNative_zlibGzip",
        move |args: Arguments| -> Option<Vec<u8>> {
            let data = args_bytes(args, ptr)?;
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&data).ok()?;
            encoder.finish().ok()
        },
    )?;
    context.add_callback(
        "__ximeNative_zlibGunzip",
        move |args: Arguments| -> Option<Vec<u8>> {
            let data = args_bytes(args, ptr)?;
            use std::io::Read;
            let mut decoder = flate2::read::GzDecoder::new(&data[..]);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).ok()?;
            Some(out)
        },
    )?;

    // ---- crypto（契约同 Android CryptoHostApi；协议插件签名用）----
    context.add_callback(
        "__ximeNative_cryptoSha256",
        move |args: Arguments| -> Vec<u8> {
            args_bytes(args, ptr)
                .map(|data| Sha256::digest(&data).to_vec())
                .unwrap_or_default()
        },
    )?;
    context.add_callback(
        "__ximeNative_cryptoHmacSha256",
        move |args: Arguments| -> Option<Vec<u8>> {
            let (key, data) = args_two_bytes(args, ptr)?;
            let mut mac = Hmac::<Sha256>::new_from_slice(&key).ok()?;
            mac.update(&data);
            Some(mac.finalize().into_bytes().to_vec())
        },
    )?;
    context.add_callback(
        "__ximeNative_cryptoHmacSha1",
        move |args: Arguments| -> Option<Vec<u8>> {
            let (key, data) = args_two_bytes(args, ptr)?;
            let mut mac = Hmac::<Sha1>::new_from_slice(&key).ok()?;
            mac.update(&data);
            Some(mac.finalize().into_bytes().to_vec())
        },
    )?;
    context.add_callback("__ximeNative_cryptoHex", move |args: Arguments| -> String {
        args_bytes(args, ptr).map(hex::encode).unwrap_or_default()
    })?;
    context.add_callback(
        "__ximeNative_cryptoBase64",
        move |args: Arguments| -> String {
            args_bytes(args, ptr)
                .map(|data| base64::engine::general_purpose::STANDARD.encode(data))
                .unwrap_or_default()
        },
    )?;
    context.add_callback("__ximeNative_cryptoUtcTime", |format: String| -> String {
        format_utc_time(&format)
    })?;
    context.add_callback("__ximeNative_cryptoEpochSeconds", || -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    })?;

    // ---- http（同步白名单请求；失败返回 null，bootstrap 包装成 XimeError；
    //      域名未授权返回 __ximeError 标记 → XimeError E_DENIED）----
    context.add_callback(
        "__ximeNative_httpRequest",
        move |args: Arguments| -> Result<OwnedJsValue, String> {
            let response = http_request_impl(args, ptr, &network, &policy_config)?;
            to_js(ptr, &response).map_err(|e| e.to_string())
        },
    )?;

    // ---- require 路径解析/读取（仅插件包内 .js，≤2MB）----
    let base = plugin_dir.clone();
    context.add_callback(
        "__ximeNative_resolveModule",
        move |spec: String| -> Option<String> {
            resolve_module(&base, &spec).map(|p| p.to_string_lossy().to_string())
        },
    )?;
    let base = plugin_dir;
    context.add_callback(
        "__ximeNative_readModule",
        move |path: String| -> Option<String> {
            let path = PathBuf::from(&path);
            if !path.is_file() || !starts_with_dir(&path, &base) {
                return None;
            }
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size > MODULE_MAX_BYTES {
                return None;
            }
            std::fs::read_to_string(&path).ok()
        },
    )?;

    // ---- polyfill 后端（utf8/base64 原语）----
    context.add_callback("__ximeNative_utf8Encode", |s: String| -> Vec<u8> {
        s.into_bytes()
    })?;
    context.add_callback(
        "__ximeNative_utf8Decode",
        move |args: Arguments| -> Option<String> { String::from_utf8(args_bytes(args, ptr)?).ok() },
    )?;
    context.add_callback(
        "__ximeNative_base64Decode",
        |s: String| -> Option<Vec<u8>> {
            base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .ok()
        },
    )?;
    context.add_callback("__ximeNative_base64Encode", |s: String| -> String {
        base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
    })?;

    Ok(())
}

fn join_args(args: Arguments) -> String {
    args.into_vec()
        .iter()
        .map(|v| v.js_to_string().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Arguments 首参 → 字节数组（bootstrap 已把 Uint8Array/字符串转为数组）。
fn args_bytes(args: Arguments, ptr: *mut q::JSContext) -> Option<Vec<u8>> {
    let first = args.into_vec().into_iter().next()?;
    from_js(ptr, &first).ok()
}

/// Arguments 前两参 → 两个字节数组（hmac key/data）。
fn args_two_bytes(args: Arguments, ptr: *mut q::JSContext) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut iter = args.into_vec().into_iter();
    let key = from_js(ptr, &iter.next()?).ok()?;
    let data = from_js(ptr, &iter.next()?).ok()?;
    Some((key, data))
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

/// host.http.request 实现（ureq 同步请求；超时参数毫秒，默认/上限见内常量）。
/// 返回 `{status, headers, body: number[], text}`；传输失败返回 Null（bootstrap
/// 包装层转 XimeError），参数非法返回 Err（直接抛 JS 异常）。
fn http_request_impl(
    args: Arguments,
    ptr: *mut q::JSContext,
    network: &NetworkDecl,
    config_file: &Path,
) -> Result<serde_json::Value, String> {
    let argv = args.into_vec();
    let get = |i: usize| -> Option<&OwnedJsValue> {
        argv.get(i).filter(|v| !v.is_undefined() && !v.is_null())
    };

    let method = get(0)
        .map(|v| v.js_to_string().unwrap_or_default())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "GET".to_string());
    let url = get(1)
        .map(|v| v.js_to_string().unwrap_or_default())
        .unwrap_or_default();
    if url.is_empty() {
        return Err("http.request: url 不能为空".to_string());
    }

    // 网络门禁：fail-closed（域名未声明且不属于用户配置的服务器 → 拒绝）
    let host = url_host(&url).ok_or_else(|| "http.request: 无法解析 URL 域名".to_string())?;
    if !network_allowed(network, config_file, &host) {
        tracing::warn!("[plugin http] 域名未授权，已拒绝: {host}");
        return Ok(serde_json::json!({
            "__ximeError": {
                "code": "E_DENIED",
                "message": format!(
                    "域名 {host} 未在 manifest network.hosts 声明，也不属于插件配置的服务器地址"
                ),
            }
        }));
    }

    let mut builder = ureq::http::Request::builder()
        .method(method.to_uppercase().as_str())
        .uri(&url);
    if let Some(headers) = get(2).filter(|v| v.is_object()) {
        let obj = headers
            .clone()
            .try_into_object()
            .map_err(|e| e.to_string())?;
        let entries: Vec<_> = obj
            .properties_iter()
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let key_str = pair[0].js_to_string().map_err(|e| e.to_string())?;
            let value_str = pair[1].js_to_string().map_err(|e| e.to_string())?;
            let Ok(key) = key_str.parse::<ureq::http::HeaderName>() else {
                continue;
            };
            let Ok(value) = value_str.parse::<ureq::http::HeaderValue>() else {
                continue;
            };
            builder = builder.header(key, value);
        }
    }

    let mut body: Vec<u8> = Vec::new();
    if let Some(v) = get(3) {
        if v.is_string() {
            body = from_js::<String>(ptr, v)
                .map_err(|e| e.to_string())?
                .into_bytes();
        } else if v.is_array() {
            body = from_js::<Vec<u8>>(ptr, v).map_err(|e| e.to_string())?;
        }
    }

    let timeout_ms = get(4)
        .and_then(|v| v.to_float().ok())
        .unwrap_or(20_000.0)
        .clamp(1.0, 120_000.0) as u64;

    let request = builder.body(body).map_err(|e| e.to_string())?;
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(timeout_ms)))
        .build()
        .new_agent();

    let response = match agent.run(request) {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("[plugin http] {method} {url}: {e}");
            return Ok(serde_json::Value::Null);
        }
    };

    let status = response.status().as_u16();
    let mut header_map = serde_json::Map::new();
    for (k, v) in response.headers() {
        header_map.insert(
            k.as_str().to_string(),
            serde_json::Value::String(v.to_str().unwrap_or_default().to_string()),
        );
    }
    let body_bytes = response.into_body().read_to_vec().unwrap_or_default();
    let text = String::from_utf8_lossy(&body_bytes).into_owned();

    Ok(serde_json::json!({
        "status": status,
        "headers": header_map,
        "body": body_bytes,
        "text": text,
    }))
}

/// 从 URL（或裸 host[:port]）提取小写域名；容忍 scheme 缺失、userinfo 与端口。
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    // IPv6 字面量 [::1]:443
    if let Some(inner) = authority.strip_prefix('[') {
        return inner.split(']').next().map(str::to_lowercase);
    }
    let host = authority.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

/// 从单个配置值推导授权域名：带 scheme 的值按 URL 解析；无 `@`/路径/空白
/// 且含 `.` 的裸值按 host[:port] 解析（用户配置的服务器地址自动授权，
/// 同 Android NetworkPolicy 第三重门；避免把邮箱形用户名误当域名）。
fn authorized_host_from_value(value: &str) -> Option<String> {
    let value = value.trim();
    let is_url = value.contains("://");
    let is_bare_host =
        !value.is_empty() && !value.contains(['@', '/', ' ', '\\']) && value.contains('.');
    if is_url || is_bare_host {
        url_host(value)
    } else {
        None
    }
}

/// 域名是否放行：manifest.network.hosts 声明，或 allowCustomHosts 且该域名
/// 出现在插件配置值中（配置文件即用户授权记录，每次请求重读以覆盖配置变更）。
fn network_allowed(network: &NetworkDecl, config_file: &Path, host: &str) -> bool {
    if network.hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
        return true;
    }
    if network.allow_custom_hosts {
        let lower = host.to_lowercase();
        return load_config(config_file)
            .map(|m| {
                m.into_values()
                    .filter_map(|v| authorized_host_from_value(&v))
                    .any(|h| h == lower)
            })
            .unwrap_or(false);
    }
    false
}

/// 宿主 snake_case clipboard profile → SDK camelCase（has_data → hasData、
/// data_name → dataName；同 Android adapter 的字段映射，其余键原样透传）。
fn clipboard_profile_to_sdk(profile: &serde_json::Value) -> serde_json::Value {
    let Some(obj) = profile.as_object() else {
        return profile.clone();
    };
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        let key = match k.as_str() {
            "has_data" => "hasData",
            "data_name" => "dataName",
            other => other,
        };
        out.insert(key.to_string(), v.clone());
    }
    serde_json::Value::Object(out)
}

/// require 模块解析：仅插件包内相对路径 `.js`。候选：`<spec>.js` /
/// `<spec>/index.js`；裸模块名（无路径分隔符）兼容旧约定回退 `libs/<spec>.js`。
/// 拒绝绝对路径与 `..` 穿越；canonicalize 复核确保落点在插件目录内（含
/// 符号链接场景）。
fn resolve_module(plugin_dir: &Path, spec: &str) -> Option<PathBuf> {
    if spec.is_empty() || spec.contains('\\') {
        return None;
    }
    let rel = spec.strip_prefix("./").unwrap_or(spec);
    let rel_path = Path::new(rel);
    if rel_path.is_absolute()
        || rel_path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    let base = plugin_dir.join(rel_path);
    let mut candidates = Vec::new();
    if base.extension().map(|e| e == "js").unwrap_or(false) {
        candidates.push(base);
    } else {
        let mut with_js = base.clone().into_os_string();
        with_js.push(".js");
        candidates.push(PathBuf::from(with_js));
        candidates.push(base.join("index.js"));
        if !rel.contains('/') {
            let libs = plugin_dir.join("libs");
            let mut with_js = libs.join(rel_path).into_os_string();
            with_js.push(".js");
            candidates.push(PathBuf::from(with_js));
            candidates.push(libs.join(rel_path).join("index.js"));
        }
    }
    let canonical_base = plugin_dir.canonicalize().ok()?;
    for candidate in candidates {
        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        if !starts_with_dir(&canonical, &canonical_base) {
            continue;
        }
        let size = std::fs::metadata(&candidate).map(|m| m.len()).unwrap_or(0);
        if candidate.is_file() && size <= MODULE_MAX_BYTES {
            return Some(candidate);
        }
    }
    None
}

/// 路径前缀检查（目录边界，按组件比较）。
fn starts_with_dir(path: &Path, base: &Path) -> bool {
    path.strip_prefix(base).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("xime_plugin_js_{label}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// kaomoji 契约插件（Android `plugins/kaomoji` 的最小 JS 形态）。
    fn extract_kaomoji(label: &str) -> PathBuf {
        let dir = temp_dir(label);
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: com.example.kaomoji\nname: Kaomoji\nversion: 2.1.0\ntype: emoji\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  var kaomojis = ["(ﾟ∀ﾟ)", "(^u^)", "ಥ_ಥ", "(・ω・)"];
  globalThis.plugin = {
    emoji: {
      listCategories: function () { return ["颜文字"]; },
      query: function (q) {
        var list = [];
        for (var i = 0; i < kaomojis.length; i++) {
          var k = kaomojis[i];
          if (!q.keyword || k.indexOf(q.keyword) !== -1) {
            list.push({ id: "k" + (i + 1), text: k });
          }
          if (list.length >= q.topK) break;
        }
        return list;
      },
    },
  };
})();
"#,
        )
        .unwrap();
        dir
    }

    /// clipboard_sync 契约插件（host.config 充当远端存储）。
    fn extract_clipboard_sync_plugin(label: &str) -> PathBuf {
        let dir = temp_dir(label);
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: com.example.clipboard_sync\nname: Test Sync\nversion: 1.0.0\ntype: clipboard_sync\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  globalThis.plugin = {
    clipboardSync: {
      push: function (profile) {
        var list = host.config.getJson("remote") || [];
        list.push(profile);
        host.config.set("remote", JSON.stringify(list));
        return true;
      },
      pull: function () {
        var raw = host.config.get("remote");
        if (raw == null) return null;
        var list = JSON.parse(raw);
        return list.length ? list[list.length - 1] : null;
      },
      test: function () { return null; },
      remoteCount: function () {
        var list = host.config.getJson("remote") || [];
        return list.length;
      },
    },
  };
})();
"#,
        )
        .unwrap();
        dir
    }

    /// backup + settings 契约插件（内存态存储；含一个应被跳过的 select 节点）。
    fn extract_backup_plugin(label: &str) -> PathBuf {
        let dir = temp_dir(label);
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: test.backup\nname: TestBackup\nversion: 1.0.0\ntype: backup\ncapabilities:\n  backup:\n    protocols:\n      - webdav\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  var store = {};
  globalThis.plugin = {
    backup: {
      push: function (args) {
        store[args.name] = args.archive;
        return { ok: true, id: args.name };
      },
      list: function () {
        var out = [];
        for (var name in store) {
          out.push({ id: name, name: name, createdAt: 1700000000, size: store[name].length });
        }
        return out;
      },
      pull: function (id) { return store[id] || null; },
      remove: function (id) { delete store[id]; return true; },
      test: function () { return null; },
    },
    settings: {
      schema: function () {
        return [
          { type: "text", key: "url", label: "地址", required: true },
          { type: "secret", key: "password", label: "密码" },
          { type: "button", key: "test", label: "测试连接" },
          { type: "select", key: "proto", options: ["webdav", "s3"] },
        ];
      },
    },
  };
})();
"#,
        )
        .unwrap();
        dir
    }

    /// tool 契约插件（panel/transform/events；内部状态供断言回读）。
    fn extract_tool_plugin(label: &str) -> PathBuf {
        let dir = temp_dir(label);
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: com.example.tool\nname: Test Tool\nversion: 1.0.0\ntype: tool\ncapabilities:\n  tool:\n    display: direct\n  candidate_transform: true\n  events:\n    - input_changed\n    - text_committed\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  var state = { lastInput: null, lastAction: null, lastItemId: null, eventLog: [] };
  globalThis.plugin = {
    panel: {
      state: function (inputText) {
        state.lastInput = inputText;
        return {
          type: "panel",
          title: "Test Tool",
          items: [
            { id: "item1", text: "Item 1" },
            { id: "item2", text: "Item 2" },
          ],
        };
      },
      onInput: function (inputText) { state.lastInput = inputText; },
      onAction: function (action) { state.lastAction = action; },
      onItemClick: function (itemId) { state.lastItemId = itemId; },
    },
    transform: {
      candidates: function (req) {
        return {
          candidates: req.candidates.map(function (c) {
            return { id: c.id, text: c.text.toUpperCase(), insertText: c.insertText };
          }),
        };
      },
    },
    events: {
      onInputChanged: function (payload) {
        state.eventLog.push({ type: "input_changed", data: payload });
      },
      onTextCommitted: function (payload) {
        state.eventLog.push({ type: "text_committed", data: payload });
      },
    },
    __state: state,
  };
})();
"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn load_and_call_kaomoji_contract() {
        let dir = extract_kaomoji("main");
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();

        let categories = runtime.get_categories();
        assert_eq!(categories, vec!["颜文字".to_string()]);

        let all = runtime.get_emojis("颜文字", "", 500);
        assert_eq!(all.len(), 4);
        assert!(all
            .iter()
            .all(|e| !e.text.is_empty() && e.category == "颜文字"));

        // topK 限制
        assert_eq!(runtime.get_emojis("颜文字", "", 3).len(), 3);

        // 搜索
        let found = runtime.get_emojis("颜文字", "ﾟ", 500);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "(ﾟ∀ﾟ)");

        // 生命周期钩子缺失时静默降级
        runtime.call_on_load();
        runtime.call_on_unload();
        drop(runtime);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sandbox_masks_dynamic_evaluation() {
        let dir = extract_kaomoji("sandbox");
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();

        let eval_type: String = runtime.context.eval_as("typeof globalThis.eval").unwrap();
        assert_eq!(eval_type, "undefined", "eval 不应存在");
        let function_type: String = runtime
            .context
            .eval_as("typeof globalThis.Function")
            .unwrap();
        assert_eq!(function_type, "undefined", "Function 不应存在");
        let host_ok: bool = runtime
            .context
            .eval_as("host.sdkVersion === '3.0.0'")
            .unwrap();
        assert!(host_ok, "host.sdkVersion 应为 3.0.0");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn host_config_and_json_roundtrip() {
        let dir = extract_kaomoji("config");
        let config_file = dir.join("config.yaml");
        let runtime = PluginRuntime::load(&dir, "main.js", &config_file).unwrap();
        let ctx = &runtime.context;

        ctx.eval(
            r#"
            host.config.set("k1", "v1");
            host.config.set("k2", "v2");
        "#,
            false,
        )
        .unwrap();
        let v1: String = ctx.eval_as("host.config.get('k1')").unwrap();
        assert_eq!(v1, "v1");
        // getJson 只解析 JSON 值（非 JSON 字符串按 Android 语义容错返回 null）
        let json_sugar: bool = ctx.eval_as("(host.config.getJson('k2') === null)").unwrap();
        assert!(json_sugar, "非 JSON 字符串应返回 null");
        ctx.eval(
            r#"host.config.set("cfg", JSON.stringify({ n: 1 }));"#,
            false,
        )
        .unwrap();
        let parsed: bool = ctx
            .eval_as("(function(){ var c = host.config.getJson('cfg'); return c !== null && c.n === 1; })()")
            .unwrap();
        assert!(parsed, "JSON 值应被 getJson 解析");

        // keys/remove
        let keys: Vec<String> = ctx.eval_as("host.config.keys()").unwrap();
        assert_eq!(keys.len(), 3, "k1/k2/cfg");
        ctx.eval("host.config.remove('k2')", false).unwrap();
        let gone: bool = ctx.eval_as("host.config.get('k2') === null").unwrap();
        assert!(gone);

        // 原生 JSON roundtrip（Lua 时代的 host.json 已被原生 JSON 取代）
        ctx.eval(
            r#"
            var s = JSON.stringify({ a: 1, b: ["x", "y"] });
            if (s !== '{"a":1,"b":["x","y"]}') throw new Error(s);
            var t = JSON.parse('{"ok":true}');
            if (t.ok !== true) throw new Error("parse");
        "#,
            false,
        )
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
            dir.join("libs/util.js"),
            "module.exports = { doubled: function (n) { return n * 2; } };",
        )
        .unwrap();

        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();
        let ctx = &runtime.context;

        let doubled: i32 = ctx
            .eval_as("var u = require('util'); u.doubled(21)")
            .unwrap();
        assert_eq!(doubled, 42);

        // 缓存：二次 require 命中同一模块
        let again: i32 = ctx.eval_as("require('util').doubled(10)").unwrap();
        assert_eq!(again, 20);

        // 路径穿越被拒绝
        let escape = ctx.eval("require('../etc/passwd')", false);
        assert!(escape.is_err());

        // 不存在的模块报错
        let missing = ctx.eval("require('nope')", false);
        assert!(missing.is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn host_crypto_and_binary_roundtrip() {
        let dir = extract_kaomoji("crypto");
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();
        let ctx = &runtime.context;

        // sha256("abc") 已知摘要（返回 Uint8Array → hex 接受）
        let sha: String = ctx
            .eval_as("host.crypto.hex(host.crypto.sha256('abc'))")
            .unwrap();
        assert_eq!(
            sha,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        // base64（Basic Auth 用）
        let b64: String = ctx.eval_as("host.crypto.base64('user:pass')").unwrap();
        assert_eq!(b64, "dXNlcjpwYXNz");

        // hmacSha256（RFC 4231 测试向量）
        let hmac: String = ctx
            .eval_as(
                "host.crypto.hex(host.crypto.hmacSha256('key', 'The quick brown fox jumps over the lazy dog'))",
            )
            .unwrap();
        assert_eq!(
            hmac,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );

        // hmacSha1（同输入的常用测试向量）
        let hmac1: String = ctx
            .eval_as(
                "host.crypto.hex(host.crypto.hmacSha1('key', 'The quick brown fox jumps over the lazy dog'))",
            )
            .unwrap();
        assert_eq!(hmac1, "de7c9b85b8b78aa6bc8a7a36f70a90701c9db4d9");

        // utcTime 格式形状
        let now: String = ctx
            .eval_as("host.crypto.utcTime('YYYYMMDDTHHMMSSZ')")
            .unwrap();
        assert_eq!(now.len(), 16);
        assert!(now.ends_with('Z') && now.as_bytes()[8] == b'T');
        let epoch: f64 = ctx.eval_as("host.crypto.epochSeconds()").unwrap();
        assert!(epoch > 1_700_000_000.0);

        // zlib/bin/utf8/base64 原语与 Uint8Array 桥
        ctx.eval(
            r#"
            var src = [0, 1, 2, 255, 128, 0];
            var gz = host.zlib.gzip(src);
            if (!(gz instanceof Uint8Array)) throw new Error("gzip 应返回 Uint8Array");
            var back = host.zlib.gunzip(gz);
            if (back.length !== src.length) throw new Error("gzip roundtrip 长度");
            for (var i = 0; i < src.length; i++) {
              if (back[i] !== src[i]) throw new Error("gzip roundtrip 内容 @" + i);
            }
            var be = host.bin.uint32be(1);
            if (be[3] !== 1 || be[2] !== 0) throw new Error("bin.uint32be");
            var enc = new TextEncoder().encode("héllo");
            if (enc.length !== 6) throw new Error("utf8 encode");
            if (new TextDecoder().decode(enc) !== "héllo") throw new Error("utf8 decode");
            if (atob(btoa("Hi!")) !== "Hi!") throw new Error("atob/btoa");
        "#,
            false,
        )
        .unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clipboard_sync_contract_roundtrip() {
        let dir = extract_clipboard_sync_plugin("roundtrip");
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();
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
        // test 返回 null → None（成功）
        assert!(runtime.test_connection().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clipboard_sync_contract_empty_pull_is_none() {
        let dir = extract_clipboard_sync_plugin("empty");
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();
        assert!(runtime.clipboard_pull().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn backup_contract_binary_roundtrip() {
        let dir = extract_backup_plugin("binary");
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();

        // 含 0 字节与多字节 UTF-8 的二进制备份包
        let archive: Vec<u8> = vec![0, 1, 2, 0xFF, 0xE4, 0xBD, 0xA0, 0x00, 0x7F];
        let result = runtime.push_backup("Xime配置-test.zip", &archive);
        assert!(result.ok, "push failed: {:?}", result.message);
        let id = result.id.expect("id must be set");

        let pulled = runtime.pull_backup(&id).expect("pull must return bytes");
        assert_eq!(pulled, archive);

        let list = runtime.list_backups().expect("list must return entries");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].size, archive.len() as i64);
        assert_eq!(list[0].created_at, 1_700_000_000);

        assert!(runtime.delete_backup(&id));
        assert!(runtime.pull_backup(&id).is_none());

        // schema：select 节点降级为文本输入，text/secret/button 三类映射保留
        let schema = runtime.get_settings_schema();
        assert_eq!(schema.len(), 4);
        assert_eq!(schema[0].key, "url");
        assert_eq!(schema[0].ftype, "text");
        assert!(schema[0].required);
        assert_eq!(schema[1].ftype, "secret");
        assert_eq!(schema[2].key, "test");
        assert_eq!(schema[2].ftype, "button");
        assert_eq!(schema[3].key, "proto");
        assert_eq!(schema[3].ftype, "text", "select 应降级为文本输入");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tool_plugin_panel_state() {
        let dir = extract_tool_plugin("panel");
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();

        let state = runtime.get_panel_state("hello").expect("panel state");
        assert_eq!(state["type"], "panel");
        assert_eq!(state["title"], "Test Tool");
        assert!(state["items"].is_array());

        let last_input: String = runtime
            .context
            .eval_as("globalThis.plugin.__state.lastInput")
            .unwrap();
        assert_eq!(last_input, "hello");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tool_plugin_panel_actions() {
        let dir = extract_tool_plugin("actions");
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();

        runtime.on_panel_input("test input");
        let last_input: String = runtime
            .context
            .eval_as("globalThis.plugin.__state.lastInput")
            .unwrap();
        assert_eq!(last_input, "test input");

        runtime.on_panel_action("open_settings");
        let last_action: String = runtime
            .context
            .eval_as("globalThis.plugin.__state.lastAction")
            .unwrap();
        assert_eq!(last_action, "open_settings");

        runtime.on_panel_item_click("item1");
        let last_item_id: String = runtime
            .context
            .eval_as("globalThis.plugin.__state.lastItemId")
            .unwrap();
        assert_eq!(last_item_id, "item1");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn candidate_transform_contract() {
        let dir = extract_tool_plugin("transform");
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();

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

        let output = runtime.transform_candidates("hello", "hello", false, &input);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].text, "HELLO");
        assert_eq!(output[1].text, "WORLD");
        assert_eq!(output[1].insert_text, Some("World".to_string()));
        assert_eq!(output[1].id, Some("2".to_string()));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn event_system_contract() {
        let dir = extract_tool_plugin("events");
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();

        runtime.send_event(
            "input_changed",
            &serde_json::json!({ "text": "hello", "timestamp": 1_234_567_890 }),
        );
        runtime.send_event("text_committed", &serde_json::json!({ "text": "done" }));

        let count: i32 = runtime
            .context
            .eval_as("globalThis.plugin.__state.eventLog.length")
            .unwrap();
        assert_eq!(count, 2);
        let first_type: String = runtime
            .context
            .eval_as("globalThis.plugin.__state.eventLog[0].type")
            .unwrap();
        assert_eq!(first_type, "input_changed");
        let second_text: String = runtime
            .context
            .eval_as("globalThis.plugin.__state.eventLog[1].data.text")
            .unwrap();
        assert_eq!(second_text, "done");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn transform_timeout_degrades_and_poisons() {
        let dir = temp_dir("poison");
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  globalThis.plugin = {
    emoji: { listCategories: function () { return ["a"]; } },
    transform: { candidates: function () { while (true) {} } },
  };
})();
"#,
        )
        .unwrap();
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();

        let input = vec![CandidateTransformItem {
            id: None,
            text: "hi".to_string(),
            insert_text: None,
            image_url: None,
        }];
        // 死循环触发 15ms 超时：原样返回输入并熔断运行时
        let output = runtime.transform_candidates("", "", false, &input);
        assert_eq!(output, input);
        assert!(runtime.poisoned.load(Ordering::Relaxed), "应已熔断");

        // 熔断后所有契约调用降级
        assert!(runtime.get_categories().is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn async_plugin_methods_settle() {
        let dir = temp_dir("async");
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  globalThis.plugin = {
    clipboardSync: {
      push: function (profile) {
        return Promise.resolve(true).then(function (ok) {
          host.config.set("last", JSON.stringify(profile));
          return ok;
        });
      },
      pull: function () { return Promise.resolve(null); },
    },
  };
})();
"#,
        )
        .unwrap();
        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();

        // 返回 Promise 的方法应被阻塞落定
        assert!(runtime.clipboard_push(&serde_json::json!({ "text": "x" })));
        let stored: String = runtime.context.eval_as("host.config.get('last')").unwrap();
        assert_eq!(stored, r#"{"text":"x"}"#);
        assert!(runtime.clipboard_pull().is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn entry_not_found_and_bad_plugin_object() {
        let dir = temp_dir("entry");
        std::fs::write(
            dir.join("main.js"),
            "globalThis.other = {}; // 未定义 plugin 导出对象",
        )
        .unwrap();
        let err = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap_err();
        assert!(matches!(err, RuntimeError::NoPluginTable));

        let err = PluginRuntime::load(&dir, "missing.js", &dir.join("config.yaml")).unwrap_err();
        assert!(matches!(err, RuntimeError::EntryMissing(_)));

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

        breaker.reset();
        assert!(!breaker.is_tripped());

        breaker.record_failure();
        breaker.record_failure();
        breaker.record_success();
        assert_eq!(breaker.failures, 0);
    }

    #[test]
    fn format_utc_time_matches_sigv4_shapes() {
        assert_eq!(
            format_utc_time_from_epoch(0, "YYYYMMDDTHHMMSSZ"),
            "19700101T000000Z"
        );
        assert_eq!(format_utc_time_from_epoch(0, "YYYYMMDD"), "19700101");
        assert_eq!(
            format_utc_time_from_epoch(1_691_755_200, "YYYYMMDDTHHMMSSZ"),
            "20230811T120000Z"
        );
        assert_eq!(
            format_utc_time_from_epoch(1_691_755_200, "YYYYMMDD"),
            "20230811"
        );
        assert_eq!(format_utc_time_from_epoch(0, "T"), "T");
    }

    #[test]
    fn event_slot_naming_matches_android() {
        assert_eq!(event_slot_name("text_committed"), "onTextCommitted");
        assert_eq!(event_slot_name("input_changed"), "onInputChanged");
        assert_eq!(event_slot_name("quick_send_changed"), "onQuickSendChanged");
    }

    #[test]
    fn entry_path_checks() {
        assert!(entry_is_safe("main.js"));
        assert!(entry_is_safe("dist/entry.js"));
        assert!(entry_is_safe("./main.js"));
        assert!(!entry_is_safe(""));
        assert!(!entry_is_safe("../evil.js"));
        assert!(!entry_is_safe("/etc/passwd"));
        assert!(!entry_is_safe("a\\b.js"));
    }

    // ---- 网络白名单（fail-closed）----

    /// http 门禁端到端：未声明域名 → XimeError E_DENIED（经 bootstrap 包装）。
    #[test]
    fn http_denied_without_host_declaration() {
        let dir = temp_dir("http_deny");
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: com.example.http\ntype: tool\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  globalThis.plugin = {
    panel: {
      state: async function (inputText) {
        try {
          await host.http.request('GET', 'https://denied.example.com/x');
          globalThis.plugin.__code = 'no-error';
        } catch (e) {
          globalThis.plugin.__code = (e && e.code) ? e.code : ('no-code:' + e);
        }
        return { items: [] };
      },
    },
    dumpCode: function () { return globalThis.plugin.__code || 'unset'; },
  };
})();
"#,
        )
        .unwrap();

        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();
        let _ = runtime.get_panel_state("");
        let code = runtime.call_plugin_fn("dumpCode", &[]).unwrap();
        assert_eq!(code, serde_json::Value::String("E_DENIED".to_string()));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn http_policy_helpers() {
        // url_host 解析
        assert_eq!(
            url_host("https://a.b.com/dav/x?y=1").as_deref(),
            Some("a.b.com")
        );
        assert_eq!(
            url_host("http://user:pw@Host.EXAMPLE.com:8080/p").as_deref(),
            Some("host.example.com")
        );
        assert_eq!(
            url_host("dav.example.com:443").as_deref(),
            Some("dav.example.com")
        );
        assert_eq!(url_host("https://[::1]:8443/x").as_deref(), Some("::1"));
        assert_eq!(url_host(""), None);

        // 授权值推导：URL / 裸域名放行；邮箱形用户名不放行
        assert_eq!(
            authorized_host_from_value("https://dav.example.com/dav/").as_deref(),
            Some("dav.example.com")
        );
        assert_eq!(
            authorized_host_from_value(" dav.example.com ").as_deref(),
            Some("dav.example.com")
        );
        assert_eq!(authorized_host_from_value("me@mail.example.com"), None);
        assert_eq!(authorized_host_from_value("password123"), None);

        let dir = temp_dir("http_policy");
        let config = dir.join("config.yaml");
        std::fs::write(
            &config,
            "url: https://dav.example.com/dav/\nuser: me@wrong.com\n",
        )
        .unwrap();

        let declared = NetworkDecl {
            hosts: vec!["api.example.org".to_string()],
            allow_custom_hosts: false,
        };
        assert!(network_allowed(&declared, &config, "api.example.org"));
        assert!(!network_allowed(&declared, &config, "dav.example.com"));
        // 子域不算命中（精确匹配）
        assert!(!network_allowed(&declared, &config, "evil.api.example.org"));

        let custom = NetworkDecl {
            hosts: vec![],
            allow_custom_hosts: true,
        };
        // 配置值中出现过的服务器域名 → 授权（用户配置即授权）
        assert!(network_allowed(&custom, &config, "dav.example.com"));
        // 邮箱形用户名不应授权其域名
        assert!(!network_allowed(&custom, &config, "wrong.com"));
        assert!(!network_allowed(&custom, &config, "other.example.com"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ---- clipboard profile 键名形态（宿主 snake_case ↔ SDK camelCase）----

    #[test]
    fn clipboard_profile_sdk_case_roundtrip() {
        let dir = temp_dir("clip_case");
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: com.example.clip\ntype: clipboard_sync\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  var seen = null;
  globalThis.plugin = {
    clipboardSync: {
      push: function (profile) {
        seen = profile;
        return typeof profile.hasData === 'boolean';
      },
      pull: function () {
        // SDK 形态（camelCase），不带 hash：宿主应补算
        return { type: 'text', text: 'hello', hasData: false, dataName: null, size: 5 };
      },
    },
    dumpSeen: function () { return seen; },
  };
})();
"#,
        )
        .unwrap();

        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();

        // push：宿主 snake_case → 插件收到 camelCase
        let pushed = runtime.clipboard_push(&serde_json::json!({
            "type": "text", "hash": "h1", "text": "hello", "has_data": false, "size": 5
        }));
        assert!(pushed);
        let seen = runtime.call_plugin_fn("dumpSeen", &[]).unwrap();
        assert_eq!(seen.get("hasData").and_then(|v| v.as_bool()), Some(false));
        assert!(
            seen.get("has_data").is_none(),
            "snake_case 键不应透传给插件"
        );

        // pull：插件 camelCase → 宿主 snake_case + hash 按 text 补算
        let profile = runtime.clipboard_pull().unwrap();
        assert_eq!(
            profile.get("has_data").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert!(
            profile.get("hasData").is_none(),
            "camelCase 键不应透传给宿主"
        );
        assert_eq!(profile.get("text").and_then(|v| v.as_str()), Some("hello"));
        let hash = profile.get("hash").and_then(|v| v.as_str()).unwrap();
        let expect = hex::encode(Sha256::digest(b"hello"));
        assert_eq!(hash, expect, "hash 缺失时应按 text 补算 sha256");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ---- 事件订阅门禁 ----

    #[test]
    fn send_event_requires_manifest_subscription() {
        let dir = temp_dir("event_gate");
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: com.example.ev\ntype: tool\ncapabilities:\n  events:\n    - input_changed\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  var log = [];
  globalThis.plugin = {
    events: {
      onInputChanged: function (p) { log.push('input_changed'); },
      onTextCommitted: function (p) { log.push('text_committed'); },
    },
    dumpLog: function () { return log; },
  };
})();
"#,
        )
        .unwrap();

        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();
        runtime.send_event("input_changed", &serde_json::json!({ "inputText": "a" }));
        runtime.send_event(
            "text_committed",
            &serde_json::json!({ "committedText": "a" }),
        );
        let log = runtime
            .call_plugin_fn("dumpLog", &[])
            .unwrap()
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(log.len(), 1, "未订阅的事件不应投递");
        assert_eq!(
            log[0],
            serde_json::Value::String("input_changed".to_string())
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ---- settings schema 降级 ----

    #[test]
    fn settings_schema_degrades_select_and_skips_unknown() {
        let dir = temp_dir("schema_degrade");
        std::fs::write(
            dir.join("manifest.yaml"),
            "id: com.example.schema\ntype: backup\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.js"),
            r#"(function () {
  globalThis.plugin = {
    settings: {
      schema: function () {
        return [
          { type: 'text', key: 'url', label: '服务器地址' },
          { type: 'secret', key: 'password', label: '密码' },
          { type: 'select', key: 'mode', label: '模式', options: ['a', 'b'] },
          { type: 'switch', key: 'auto', label: '自动' },
          { type: 'section', label: '分组标题' },
          { type: 'mystery', key: 'weird', label: '未知类型' },
          { type: 'button', key: 'testConnection', label: '测试连接' },
        ];
      },
    },
  };
})();
"#,
        )
        .unwrap();

        let runtime = PluginRuntime::load(&dir, "main.js", &dir.join("config.yaml")).unwrap();
        let fields = runtime.get_settings_schema();
        let keys: Vec<(&str, &str)> = fields
            .iter()
            .map(|f| (f.key.as_str(), f.ftype.as_str()))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("url", "text"),
                ("password", "secret"),
                ("mode", "text"), // select 降级为文本输入
                ("auto", "text"), // switch 降级为文本输入
                ("testConnection", "button"),
            ],
            "无 key 的 section 与未知类型节点应被过滤"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 真实安装插件的冒烟验证（需要本机数据目录有 Android xipm 构建产物）：
    /// `cargo test -p xime-plugin --lib -- --ignored real_plugins_smoke`
    #[test]
    #[ignore = "依赖本机 ~/Library/Application Support/XimeYi/plugins 的真实插件"]
    fn real_plugins_smoke() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let base = PathBuf::from(home).join("Library/Application Support/XimeYi/plugins");

        // kaomoji：emoji 契约（分类 + 查询）
        let kaomoji = base.join("com.kingzcheung.xime.plugin.kaomoji");
        if kaomoji.join("main.js").is_file() {
            let runtime = PluginRuntime::load(
                &kaomoji,
                "main.js",
                &base.join("config/com.kingzcheung.xime.plugin.kaomoji.yaml"),
            )
            .expect("kaomoji 加载失败");
            let categories = runtime.get_categories();
            assert!(!categories.is_empty(), "kaomoji 分类为空");
            let items = runtime.get_emojis(&categories[0], "", 10);
            assert!(!items.is_empty(), "kaomoji 表情为空");
            let manifest = PluginManifest::from_dir(&kaomoji).unwrap();
            assert_eq!(manifest.id, "com.kingzcheung.xime.plugin.kaomoji");
        } else {
            eprintln!("跳过：未找到 JS 版 kaomoji（{kaomoji:?}）");
        }

        // webdav-backup：settings schema 扁平化（真实 XimeUiNode 形态）
        let webdav = base.join("com.kingzcheung.xime.plugin.webdav_backup");
        if webdav.join("main.js").is_file() {
            let runtime = PluginRuntime::load(
                &webdav,
                "main.js",
                &base.join("config/com.kingzcheung.xime.plugin.webdav_backup.yaml"),
            )
            .expect("webdav-backup 加载失败");
            let schema = runtime.get_settings_schema();
            let keys: Vec<&str> = schema.iter().map(|f| f.key.as_str()).collect();
            assert!(keys.contains(&"url"), "schema 应含 url 字段: {keys:?}");
            assert!(
                keys.contains(&"password"),
                "schema 应含 password 字段: {keys:?}"
            );
            assert!(
                keys.contains(&"testConnection"),
                "schema 应含测试连接按钮: {keys:?}"
            );
        } else {
            eprintln!("跳过：未找到 JS 版 webdav-backup（{webdav:?}）");
        }
    }
}
