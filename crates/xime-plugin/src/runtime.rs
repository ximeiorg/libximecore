use mlua::prelude::*;
use mlua::{Function, LuaOptions, MultiValue, StdLib, Table, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

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
}
