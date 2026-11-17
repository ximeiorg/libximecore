//! 导入词库：把外部词库转换成派生 `.dict.yaml`，再通过
//! `<方案>.custom.yaml` 的 `translator/packs` 挂到方案上。
//!
//! 不改用户已有的 `.dict.yaml` / `.schema.yaml`，也不写 userdb/leveldb。
//! 具体格式由 `xime-lexicon` 的解析器负责；本模块只处理
//! 「统一词条模型 → 派生词典 → packs」。
//!
//! 记录写在统一库 `xime.db` 的 `imported_lexicons` 表（与 Android 同表同列），
//! 派生词典与方案同目录，交给 Rime 部署编译。旧版
//! `<用户目录>/imported_dicts/manifest.json` 由 [`migrate_manifest_file`]
//! 一次性搬进表并改名为 `.migrated`。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use xime_lexicon::{CodeSystem, Entry, LexiconError, ParsedLexicon};

/// 派生词典文件名前缀（和 Rime 自带词库区分开）。
const PACK_PREFIX: &str = "xime_import_";
const MANIFEST_DIR: &str = "imported_dicts";
const MANIFEST_FILE: &str = "manifest.json";
/// packs 托管块的起止标记：重写时只替换这两行之间的内容，用户自己的 patch 保留。
const BLOCK_START: &str = "# >>> xime-imported-dicts";
const BLOCK_END: &str = "# <<< xime-imported-dicts";

/// 一本已导入的词库。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedDict {
    /// 派生词典名（`<id>.dict.yaml`），同时是 packs 里引用的名字。
    pub id: String,
    /// 展示名（取自源文件名）。
    pub display_name: String,
    pub entry_count: usize,
    /// 挂载的方案；空 = 只生成文件，没挂到任何方案。
    #[serde(default)]
    pub attached_schemas: Vec<String>,
    /// 方案 → 翻译器命名空间（`translator` / 其他 translator 的 `@` 后缀）。
    #[serde(default)]
    pub namespaces: BTreeMap<String, String>,
    /// 导入时间（毫秒时间戳）。
    #[serde(default)]
    pub created_at: i64,
    /// 解析器格式 id。
    #[serde(default)]
    pub format_id: String,
    /// 解析器格式名（如「搜狗细胞词库」），仅用于展示。
    #[serde(default)]
    pub format_name: String,
    /// 用户选择的原文件名。
    #[serde(default)]
    pub source_name: String,
    /// 编码体系名（`Pinyin` / `Wubi` / `Other`），仅用于展示。
    #[serde(default = "default_code_system")]
    pub code_system: String,
    /// 停用后保留文件和记录，只从 packs 托管块里摘掉。
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

fn default_code_system() -> String {
    CodeSystem::Pinyin.label().to_string()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    items: Vec<ImportedDict>,
}

/// 方案的翻译器信息：组件名 + 命名空间（`@` 之后的部分）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslatorInfo {
    pub component: String,
    pub namespace: String,
}

impl TranslatorInfo {
    /// 该翻译器是否吃这个编码体系：拼音挂 `script_translator`，
    /// 其余（五笔/笔画）挂 `table_translator`。
    pub fn accepts(&self, code_system: CodeSystem) -> bool {
        if code_system == CodeSystem::Pinyin {
            self.component == "script_translator"
        } else {
            self.component == "table_translator"
        }
    }
}

// ------------------------------------------------------------------
// 路径与清单
// ------------------------------------------------------------------

/// 清单文件路径。
pub fn manifest_path(user_dir: &Path) -> PathBuf {
    user_dir.join(MANIFEST_DIR).join(MANIFEST_FILE)
}

/// 派生词典路径。
pub fn dict_path(user_dir: &Path, id: &str) -> PathBuf {
    user_dir.join(format!("{id}.dict.yaml"))
}

/// 记录所在的统一库路径（Rime 用户目录的同级 `xime.db`）。
pub fn db_path(user_dir: &Path) -> PathBuf {
    crate::app_db::db_path_beside(user_dir)
}

/// 读清单；库打不开或记录损坏都按空清单处理（不让存储问题卡住整个页面）。
pub fn list(user_dir: &Path) -> Vec<ImportedDict> {
    match open_db(user_dir).and_then(|conn| load_all(&conn)) {
        Ok(items) => items,
        Err(error) => {
            tracing::warn!("[imported_dict] 读取导入记录失败：{error}");
            Vec::new()
        }
    }
}

/// 打开统一库，顺手把旧 `manifest.json` 搬进表（幂等）。
fn open_db(user_dir: &Path) -> Result<Connection, String> {
    let conn = crate::app_db::open(&db_path(user_dir))
        .map_err(|e| format!("打开数据库失败：{e}"))?;
    migrate_manifest_file(user_dir, &conn)?;
    Ok(conn)
}

/// 旧版 `manifest.json` → `imported_lexicons` 表，只跑一次。
///
/// 表里已有记录就不搬（否则会把用户新导入的盖掉）；文件解析失败也保留原样，
/// 交给用户自己看——反正读出来是空清单，页面不会崩。
fn migrate_manifest_file(user_dir: &Path, conn: &Connection) -> Result<(), String> {
    let path = manifest_path(user_dir);
    if !path.is_file() {
        return Ok(());
    }
    let existing: i64 = conn
        .query_row("SELECT COUNT(*) FROM imported_lexicons", [], |row| row.get(0))
        .map_err(|e| format!("统计导入记录失败：{e}"))?;
    if existing > 0 {
        return Ok(());
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let Ok(manifest) = serde_json::from_str::<Manifest>(&text) else {
        tracing::warn!(
            "[imported_dict] 旧清单解析失败，保留原文件 {}",
            path.display()
        );
        return Ok(());
    };
    if manifest.items.is_empty() {
        return Ok(());
    }
    write_all(conn, &manifest.items)?;
    // 改名而不是删：万一迁移字段有偏差，用户还能找回原文件。
    let _ = std::fs::rename(&path, path.with_extension("json.migrated"));
    tracing::info!(
        "[imported_dict] 已把 {} 条导入记录从 manifest.json 迁入 xime.db",
        manifest.items.len()
    );
    Ok(())
}

fn load_all(conn: &Connection) -> Result<Vec<ImportedDict>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, displayName, formatId, formatName, codeSystem, schemes,
                    entryCount, enabled, sourceName, createdAt
             FROM imported_lexicons
             ORDER BY createdAt DESC, id DESC",
        )
        .map_err(|e| format!("查询导入记录失败：{e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let schemes: String = row.get(5)?;
            let (attached_schemas, namespaces) = decode_schemes(&schemes);
            Ok(ImportedDict {
                id: row.get(0)?,
                display_name: row.get(1)?,
                entry_count: row.get::<_, i64>(6)?.max(0) as usize,
                attached_schemas,
                namespaces,
                created_at: row.get(9)?,
                format_id: row.get(2)?,
                format_name: row.get(3)?,
                source_name: row.get(8)?,
                code_system: row.get(4)?,
                enabled: row.get::<_, i64>(7)? != 0,
            })
        })
        .map_err(|e| format!("查询导入记录失败：{e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取导入记录失败：{e}"))
}

/// 整份重写记录（数据量只有几本~几十本，事务内清空再插最省心）。
fn save_manifest(user_dir: &Path, items: &[ImportedDict]) -> Result<(), String> {
    let conn = open_db(user_dir)?;
    write_all(&conn, items)
}

fn write_all(conn: &Connection, items: &[ImportedDict]) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("开启事务失败：{e}"))?;
    tx.execute("DELETE FROM imported_lexicons", [])
        .map_err(|e| format!("清空导入记录失败：{e}"))?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT OR REPLACE INTO imported_lexicons
                 (id, displayName, dictFile, formatId, formatName, codeSystem, schemes,
                  entryCount, enabled, sourceName, createdAt)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )
            .map_err(|e| format!("准备写入记录失败：{e}"))?;
        for item in items {
            stmt.execute(params![
                item.id.as_str(),
                item.display_name.as_str(),
                format!("{}.dict.yaml", item.id),
                item.format_id.as_str(),
                item.format_name.as_str(),
                item.code_system.as_str(),
                encode_schemes(&item.attached_schemas, &item.namespaces),
                item.entry_count as i64,
                i64::from(item.enabled),
                item.source_name.as_str(),
                item.created_at,
            ])
            .map_err(|e| format!("写入记录 {} 失败：{e}", item.id))?;
        }
    }
    tx.commit().map_err(|e| format!("提交导入记录失败：{e}"))
}

/// 方案绑定序列化成 `schemaId=namespace;...`（与 Android `encodeSchemes` 同格式）。
fn encode_schemes(schemas: &[String], namespaces: &BTreeMap<String, String>) -> String {
    schemas
        .iter()
        .map(|id| {
            format!(
                "{id}={}",
                namespaces
                    .get(id)
                    .map(String::as_str)
                    .unwrap_or("translator")
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// `schemaId=namespace;...` → (方案列表, 命名空间表)。
fn decode_schemes(value: &str) -> (Vec<String>, BTreeMap<String, String>) {
    let mut schemas = Vec::new();
    let mut namespaces = BTreeMap::new();
    for part in value.split(';') {
        let (schema_id, namespace) = match part.split_once('=') {
            Some((id, ns)) => (id.trim(), ns.trim()),
            None => (part.trim(), "translator"),
        };
        if schema_id.is_empty() {
            continue;
        }
        schemas.push(schema_id.to_string());
        namespaces.insert(
            schema_id.to_string(),
            if namespace.is_empty() {
                "translator".to_string()
            } else {
                namespace.to_string()
            },
        );
    }
    (schemas, namespaces)
}

// ------------------------------------------------------------------
// 方案翻译器探测
// ------------------------------------------------------------------

/// 从 `.schema.yaml` 文本里找主翻译器（第一个 table/script translator）。
pub fn main_translator(schema_text: &str) -> Option<TranslatorInfo> {
    let value: serde_yaml::Value = serde_yaml::from_str(schema_text).ok()?;
    let translators = value.get("engine")?.get("translators")?.as_sequence()?;
    for item in translators {
        let item = item.as_str()?.trim();
        if item.is_empty() {
            continue;
        }
        let component = item.split('@').next().unwrap_or(item).trim();
        if component != "table_translator" && component != "script_translator" {
            continue;
        }
        let namespace = item
            .split_once('@')
            .map(|(_, namespace)| namespace.trim().to_string())
            .filter(|namespace| !namespace.is_empty())
            .unwrap_or_else(|| "translator".to_string());
        return Some(TranslatorInfo {
            component: component.to_string(),
            namespace,
        });
    }
    None
}

/// 读某方案的翻译器信息；方案文件不存在或没有翻译器时返回 `None`。
pub fn schema_translator(user_dir: &Path, schema_id: &str) -> Option<TranslatorInfo> {
    let path = user_dir.join(format!("{schema_id}.schema.yaml"));
    let text = std::fs::read_to_string(path).ok()?;
    main_translator(&text)
}

/// 该方案能否挂这种编码体系的词库。
pub fn schema_accepts(user_dir: &Path, schema_id: &str, code_system: CodeSystem) -> bool {
    schema_translator(user_dir, schema_id).is_some_and(|info| info.accepts(code_system))
}

// ------------------------------------------------------------------
// 派生词典
// ------------------------------------------------------------------

/// 生成派生 `.dict.yaml` 的文本。
pub fn dict_yaml_text(id: &str, entries: &[Entry], source_name: &str) -> String {
    let mut out = String::with_capacity(entries.len() * 16 + 160);
    out.push_str("# Rime dictionary\n# encoding: utf-8\n");
    let source = sanitize_comment(source_name);
    if !source.is_empty() {
        out.push_str("# imported from: ");
        out.push_str(&source);
        out.push('\n');
    }
    out.push_str("---\n");
    out.push_str(&format!("name: {id}\n"));
    out.push_str("version: \"1\"\n");
    out.push_str("sort: by_weight\n");
    out.push_str("use_preset_vocabulary: false\n");
    out.push_str("...\n");
    for entry in entries {
        out.push_str(&entry.word);
        out.push('\t');
        out.push_str(&entry.code);
        out.push('\t');
        out.push_str(&entry.weight.to_string());
        out.push('\n');
    }
    out
}

fn sanitize_comment(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .collect::<String>()
        .trim()
        .to_string()
}

// ------------------------------------------------------------------
// custom.yaml 的 packs 托管块
// ------------------------------------------------------------------

/// 移除托管块（保留用户自己的内容）。
pub fn remove_managed_block(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == BLOCK_START {
            inside = true;
            continue;
        }
        if inside {
            if trimmed == BLOCK_END {
                inside = false;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    // 折掉连续空行与尾部空白，避免反复重写时文件越来越长。
    let mut cleaned = String::with_capacity(out.len());
    let mut blank_run = 0usize;
    for line in out.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }
    cleaned.trim_end().to_string()
}

/// 写入/替换 app 托管的 packs 块，保留 custom.yaml 里用户自己的内容。
///
/// `pack_names` 为空时只移除托管块。
pub fn upsert_managed_pack_block(original: &str, namespace: &str, pack_names: &[String]) -> String {
    let without = remove_managed_block(original);
    if pack_names.is_empty() {
        return if without.trim().is_empty() {
            String::new()
        } else {
            format!("{without}\n")
        };
    }

    let key = if namespace == "translator" || namespace.is_empty() {
        "\"translator/packs/+\"".to_string()
    } else {
        format!("\"{namespace}/packs/+\"")
    };
    let mut block = String::new();
    block.push_str("  ");
    block.push_str(BLOCK_START);
    block.push('\n');
    block.push_str("  ");
    block.push_str(&key);
    block.push_str(":\n");
    let mut seen = std::collections::HashSet::new();
    for name in pack_names {
        if seen.insert(name.as_str()) {
            block.push_str("    - ");
            block.push_str(name);
            block.push('\n');
        }
    }
    block.push_str("  ");
    block.push_str(BLOCK_END);

    if without.trim().is_empty() {
        return format!("patch:\n{block}\n");
    }

    // `patch: {}` 是空 patch 的另一种写法，直接换成 `patch:` 再插块。
    let mut base = without.clone();
    let mut inserted = false;
    let mut lines: Vec<String> = Vec::new();
    for line in base.lines() {
        let trimmed = line.trim();
        if !inserted && trimmed == "patch: {}" {
            lines.push("patch:".to_string());
            lines.push(block.clone());
            inserted = true;
            continue;
        }
        lines.push(line.to_string());
        if !inserted {
            let is_patch_head = trimmed.starts_with("patch:")
                && (trimmed.len() == 6 || trimmed.starts_with("patch: #"));
            if is_patch_head {
                lines.push(block.clone());
                inserted = true;
            }
        }
    }
    base = lines.join("\n");
    if !inserted {
        base.push_str("\n\npatch:\n");
        base.push_str(&block);
    }
    format!("{base}\n")
}

// ------------------------------------------------------------------
// 导入
// ------------------------------------------------------------------

/// 一次导入的结果。
#[derive(Debug, Clone)]
pub struct ImportOutcome {
    pub record: ImportedDict,
    pub dict_path: PathBuf,
}

/// 导入失败的原因（`message` 直接可以展示给用户）。
#[derive(Debug, Clone)]
pub struct ImportError {
    pub message: String,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ImportError {}

impl From<LexiconError> for ImportError {
    fn from(error: LexiconError) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

fn fail(message: impl Into<String>) -> ImportError {
    ImportError {
        message: message.into(),
    }
}

/// 解析一个文件，供界面预览（不写任何文件）。
pub fn parse_preview(path: &Path) -> Result<ParsedLexicon, ImportError> {
    let parsed = xime_lexicon::parse_file(path)?;
    if parsed.entries.is_empty() {
        return Err(fail("这个文件里没有可用的词条"));
    }
    Ok(parsed)
}

/// 导入一本词库：解析 → 写派生词典 → 更新清单 → 写 packs。
///
/// `scheme_ids` 为空表示只生成 `.dict.yaml`（不挂任何方案）。
/// 任一步失败都会回滚已写入的文件，不留半成品。
pub fn import_file(
    user_dir: &Path,
    source: &Path,
    scheme_ids: &[String],
) -> Result<ImportOutcome, ImportError> {
    let parsed = parse_preview(source)?;
    import_parsed(user_dir, &parsed, source, scheme_ids)
}

/// 同 [`import_file`]，但用已解析好的结果（预览过的文件不必再解析一遍）。
pub fn import_parsed(
    user_dir: &Path,
    parsed: &ParsedLexicon,
    source: &Path,
    scheme_ids: &[String],
) -> Result<ImportOutcome, ImportError> {
    if parsed.entries.is_empty() {
        return Err(fail("这个文件里没有可用的词条"));
    }
    if parsed.code_system != CodeSystem::Pinyin {
        return Err(fail(format!(
            "暂不支持{}编码的词库，只能导入拼音词库",
            parsed.code_system.label()
        )));
    }

    // 方案必须先验证：写了一半再发现方案不吃这种编码就很尴尬。
    let mut namespaces = BTreeMap::new();
    let mut attached = Vec::new();
    for schema_id in scheme_ids.iter().map(|id| id.trim()).filter(|id| !id.is_empty()) {
        if attached.iter().any(|existing: &String| existing == schema_id) {
            continue;
        }
        let translator = schema_translator(user_dir, schema_id).ok_or_else(|| {
            fail(format!(
                "方案 {schema_id} 没有可用的翻译器（方案可能未安装）"
            ))
        })?;
        if !translator.accepts(parsed.code_system) {
            return Err(fail(format!(
                "方案 {schema_id} 的翻译器是 {}，挂不了{}词库",
                translator.component,
                parsed.code_system.label()
            )));
        }
        namespaces.insert(schema_id.to_string(), translator.namespace);
        attached.push(schema_id.to_string());
    }

    let existing = list(user_dir);
    let id = new_id(user_dir);
    let source_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("导入词库")
        .to_string();
    let record = ImportedDict {
        id: id.clone(),
        display_name: display_name(&source_name),
        entry_count: parsed.entries.len(),
        attached_schemas: attached.clone(),
        namespaces,
        created_at: now_millis(),
        format_id: parsed.format_id.to_string(),
        format_name: parsed.format_name.to_string(),
        source_name,
        code_system: parsed.code_system.label().to_string(),
        // 未挂载的导入先停用：没有方案可生效时不该显示成「已启用」。
        enabled: !attached.is_empty(),
    };

    let dict_file = dict_path(user_dir, &id);
    let yaml = dict_yaml_text(&id, &parsed.entries, &record.source_name);
    std::fs::write(&dict_file, yaml)
        .map_err(|e| fail(format!("写派生词典 {} 失败：{e}", dict_file.display())))?;

    let mut next = existing.clone();
    next.push(record.clone());
    let backups = match apply_packs(user_dir, &next, &attached, &record.namespaces) {
        Ok(backups) => backups,
        Err(error) => {
            let _ = std::fs::remove_file(&dict_file);
            return Err(error);
        }
    };
    if let Err(error) = save_manifest(user_dir, &next) {
        restore_custom_files(&backups);
        let _ = std::fs::remove_file(&dict_file);
        return Err(fail(error));
    }

    Ok(ImportOutcome {
        record,
        dict_path: dict_file,
    })
}

/// 启用/停用：停用只摘 packs，保留派生词典和记录。
pub fn set_enabled(user_dir: &Path, id: &str, enabled: bool) -> Result<(), String> {
    let mut items = list(user_dir);
    let index = items
        .iter()
        .position(|item| item.id == id)
        .ok_or_else(|| format!("词库记录不存在：{id}"))?;
    if enabled && items[index].attached_schemas.is_empty() {
        return Err("这本词库还没有挂到任何方案".to_string());
    }
    items[index].enabled = enabled;
    let record = items[index].clone();
    let backups = apply_packs_to_record(user_dir, &items, &record)?;
    if let Err(error) = save_manifest(user_dir, &items) {
        restore_custom_files(&backups);
        return Err(error);
    }
    Ok(())
}

/// 删除：摘 packs、删派生词典、删记录。
pub fn remove(user_dir: &Path, id: &str) -> Result<(), String> {
    let items = list(user_dir);
    let record = items
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| format!("词库记录不存在：{id}"))?;
    let remaining: Vec<ImportedDict> = items.iter().filter(|item| item.id != id).cloned().collect();

    let backups = apply_packs(
        user_dir,
        &remaining,
        &record.attached_schemas,
        &record.namespaces,
    )
    .map_err(|error| error.message)?;

    let file = dict_path(user_dir, id);
    let backup = std::fs::read(&file).ok();
    if file.exists() {
        if let Err(error) = std::fs::remove_file(&file) {
            restore_custom_files(&backups);
            return Err(format!("删除派生词典失败：{error}"));
        }
    }
    if let Err(error) = save_manifest(user_dir, &remaining) {
        if let Some(bytes) = backup {
            let _ = std::fs::write(&file, bytes);
        }
        restore_custom_files(&backups);
        return Err(error);
    }
    Ok(())
}

/// 改挂方案：旧方案不在新集合里就摘掉。
pub fn rebind(user_dir: &Path, id: &str, scheme_ids: &[String]) -> Result<(), String> {
    let mut items = list(user_dir);
    let index = items
        .iter()
        .position(|item| item.id == id)
        .ok_or_else(|| format!("词库记录不存在：{id}"))?;
    let code_system = code_system_of(&items[index]);

    let mut namespaces = BTreeMap::new();
    let mut attached = Vec::new();
    for schema_id in scheme_ids.iter().map(|id| id.trim()).filter(|id| !id.is_empty()) {
        let translator = schema_translator(user_dir, schema_id)
            .ok_or_else(|| format!("方案 {schema_id} 没有可用的翻译器"))?;
        if !translator.accepts(code_system) {
            return Err(format!(
                "方案 {schema_id} 的翻译器是 {}，挂不了{}词库",
                translator.component,
                code_system.label()
            ));
        }
        namespaces.insert(schema_id.to_string(), translator.namespace);
        attached.push(schema_id.to_string());
    }
    if attached.is_empty() {
        return Err("至少要选一个方案".to_string());
    }

    let old = items[index].clone();
    items[index].attached_schemas = attached.clone();
    items[index].namespaces = namespaces;
    // 首次给未挂载词库绑定方案时自动启用。
    if old.attached_schemas.is_empty() {
        items[index].enabled = true;
    }

    let affected: Vec<String> = old
        .attached_schemas
        .iter()
        .chain(attached.iter())
        .cloned()
        .collect();
    let mut hints = old.namespaces.clone();
    hints.extend(items[index].namespaces.clone());
    let backups = apply_packs(user_dir, &items, &affected, &hints).map_err(|error| error.message)?;
    if let Err(error) = save_manifest(user_dir, &items) {
        restore_custom_files(&backups);
        return Err(error);
    }
    Ok(())
}

fn code_system_of(record: &ImportedDict) -> CodeSystem {
    match record.code_system.as_str() {
        "五笔" => CodeSystem::Wubi,
        "其他" => CodeSystem::Other,
        _ => CodeSystem::Pinyin,
    }
}

/// 单个记录的 packs 重写（只影响它自己挂的方案）。
fn apply_packs_to_record(
    user_dir: &Path,
    items: &[ImportedDict],
    record: &ImportedDict,
) -> Result<Vec<(PathBuf, Option<String>)>, String> {
    apply_packs(
        user_dir,
        items,
        &record.attached_schemas,
        &record.namespaces,
    )
    .map_err(|error| error.message)
}

type CustomBackup = (PathBuf, Option<String>);

/// 按期望的清单重写各方案的 packs 托管块；返回原文件内容用于回滚。
fn apply_packs(
    user_dir: &Path,
    items: &[ImportedDict],
    scheme_ids: &[String],
    namespace_hints: &BTreeMap<String, String>,
) -> Result<Vec<CustomBackup>, ImportError> {
    let mut backups: Vec<CustomBackup> = Vec::new();
    let mut written: Vec<String> = Vec::new();

    for scheme_id in scheme_ids.iter().map(|id| id.trim()).filter(|id| !id.is_empty()) {
        if written.iter().any(|existing| existing == scheme_id) {
            continue;
        }
        written.push(scheme_id.to_string());

        let custom = user_dir.join(format!("{scheme_id}.custom.yaml"));
        let original = std::fs::read_to_string(&custom).ok();
        // 方案本身不在了就别凭空造 custom.yaml（只更新已存在的文件）。
        if original.is_none() && !user_dir.join(format!("{scheme_id}.schema.yaml")).exists() {
            continue;
        }

        let namespace = namespace_hints
            .get(scheme_id)
            .or_else(|| {
                items
                    .iter()
                    .find(|item| item.attached_schemas.iter().any(|id| id == scheme_id))
                    .and_then(|item| item.namespaces.get(scheme_id))
            })
            .cloned()
            .unwrap_or_else(|| "translator".to_string());

        let packs: Vec<String> = items
            .iter()
            .filter(|item| {
                item.enabled
                    && item.attached_schemas.iter().any(|id| id == scheme_id)
                    && item
                        .namespaces
                        .get(scheme_id)
                        .cloned()
                        .unwrap_or_else(|| "translator".to_string())
                        == namespace
            })
            .map(|item| item.id.clone())
            .collect();

        if original.is_none() && packs.is_empty() {
            continue;
        }

        let text = upsert_managed_pack_block(original.as_deref().unwrap_or(""), &namespace, &packs);
        backups.push((custom.clone(), original));
        if let Err(error) = std::fs::write(&custom, text) {
            restore_custom_files(&backups);
            return Err(fail(format!("写 {} 失败：{error}", custom.display())));
        }
    }

    Ok(backups)
}

fn restore_custom_files(backups: &[CustomBackup]) {
    for (path, original) in backups {
        match original {
            Some(text) => {
                let _ = std::fs::write(path, text);
            }
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// 生成不冲突的派生词典名。
fn new_id(user_dir: &Path) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for attempt in 0..32u32 {
        let seed = format!("{nanos}|{attempt}|{}", std::process::id());
        let digest = sha256_hex(seed.as_bytes());
        let id = format!("{PACK_PREFIX}{}", &digest[..8]);
        if !dict_path(user_dir, &id).exists() {
            return id;
        }
    }
    format!("{PACK_PREFIX}{nanos:032x}")
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 从源文件名取展示名：去掉目录与扩展名。
fn display_name(source_name: &str) -> String {
    let base = source_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(source_name);
    let without_dict_suffix = base.strip_suffix(".dict.yaml").unwrap_or(base);
    let name = match without_dict_suffix.rsplit_once('.') {
        Some((stem, _)) => stem,
        None => without_dict_suffix,
    };
    let name = name.trim();
    if name.is_empty() {
        "导入词库".to_string()
    } else {
        name.chars().take(60).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// 测试用的 Rime 用户目录：放在临时目录的 `rime/` 子目录下。
    ///
    /// 统一库落在用户目录的**同级**（`xime.db`），所以不能让 root 直接就是
    /// 临时目录——那样所有测试的库都会落到系统的 `/tmp/xime.db` 上互相污染
    /// （第一次改表时确实踩到了：id 冲突、`list()` 读到别的测试的记录）。
    fn rime_root(dir: &tempfile::TempDir) -> PathBuf {
        let root = dir.path().join("rime");
        std::fs::create_dir_all(&root).expect("mkdir rime");
        root
    }

    fn write_schema(dir: &Path, id: &str, translators: &[&str]) {
        let list = translators
            .iter()
            .map(|item| format!("      - {item}\n"))
            .collect::<String>();
        let text = format!(
            "schema:\n  schema_id: {id}\n  name: 测试方案\nengine:\n  translators:\n{list}"
        );
        std::fs::write(dir.join(format!("{id}.schema.yaml")), text).expect("write schema");
    }

    fn entry(word: &str, code: &str) -> Entry {
        Entry {
            word: word.to_string(),
            code: code.to_string(),
            weight: 1,
        }
    }

    fn parsed(entries: Vec<Entry>) -> ParsedLexicon {
        ParsedLexicon {
            format_id: "sogou-scel",
            format_name: "搜狗细胞词库",
            code_system: CodeSystem::Pinyin,
            entries,
            skipped: 0,
            duplicates: 0,
        }
    }

    #[test]
    fn dict_yaml_has_rime_header_and_entries() {
        let text = dict_yaml_text("xime_import_abc", &[entry("你好", "ni hao")], "a.scel\nb");
        assert!(text.starts_with("# Rime dictionary\n"));
        assert!(text.contains("# imported from: a.scel b\n"));
        assert!(text.contains("name: xime_import_abc\n"));
        assert!(text.contains("sort: by_weight\n"));
        assert!(text.contains("...\n"));
        assert!(text.ends_with("你好\tni hao\t1\n"));
    }

    #[test]
    fn managed_block_is_upserted_and_removed() {
        let empty = upsert_managed_pack_block("", "translator", &["a".to_string()]);
        assert_eq!(
            empty,
            "patch:\n  # >>> xime-imported-dicts\n  \"translator/packs/+\":\n    - a\n  # <<< xime-imported-dicts\n"
        );

        let custom = "patch:\n  \"menu/page_size\": 7\n";
        let with_block = upsert_managed_pack_block(custom, "translator", &["a".to_string(), "b".to_string()]);
        assert!(with_block.contains("\"menu/page_size\": 7"));
        assert!(with_block.contains("    - a\n    - b\n"));

        // 再写一次不会叠加块。
        let again = upsert_managed_pack_block(&with_block, "translator", &["a".to_string()]);
        assert_eq!(again.matches(BLOCK_START).count(), 1);
        assert!(again.contains("    - a\n") && !again.contains("    - b\n"));
        assert!(again.contains("\"menu/page_size\": 7"));

        let removed = remove_managed_block(&with_block);
        assert!(!removed.contains(BLOCK_START));
        assert!(removed.contains("\"menu/page_size\": 7"));
    }

    #[test]
    fn managed_block_handles_inline_empty_patch() {
        let text = upsert_managed_pack_block("patch: {}\n", "translator", &["p".to_string()]);
        assert!(text.contains("patch:\n  # >>> xime-imported-dicts"));
        assert!(!text.contains("patch: {}"));
    }

    #[test]
    fn main_translator_reads_first_table_or_script() {
        let script = "engine:\n  translators:\n    - script_translator@translator\n";
        let info = main_translator(script).expect("script");
        assert_eq!(info.component, "script_translator");
        assert_eq!(info.namespace, "translator");

        let table = "engine:\n  translators:\n    - punct_translator\n    - table_translator@wubi\n";
        let info = main_translator(table).expect("table");
        assert_eq!(info.component, "table_translator");
        assert_eq!(info.namespace, "wubi");

        assert!(main_translator("engine:\n  translators:\n    - punct_translator\n").is_none());
        assert!(main_translator("not: [valid").is_none());
    }

    #[test]
    fn code_system_matching() {
        let script = TranslatorInfo {
            component: "script_translator".to_string(),
            namespace: "translator".to_string(),
        };
        assert!(script.accepts(CodeSystem::Pinyin));
        assert!(!script.accepts(CodeSystem::Wubi));
    }

    #[test]
    fn import_writes_dict_and_packs() {
        let dir = temp_dir();
        let root_path = rime_root(&dir);
        let root = root_path.as_path();
        write_schema(root, "pinyin", &["script_translator@translator"]);

        let outcome = import_parsed(
            root,
            &parsed(vec![entry("你好", "ni hao"), entry("世界", "shi jie")]),
            Path::new("/tmp/网络流行语.scel"),
            &["pinyin".to_string()],
        )
        .expect("import");

        assert_eq!(outcome.record.display_name, "网络流行语");
        assert_eq!(outcome.record.entry_count, 2);
        assert!(outcome.record.enabled);
        assert_eq!(
            outcome.record.namespaces.get("pinyin").map(String::as_str),
            Some("translator")
        );

        let dict = std::fs::read_to_string(dict_path(root, &outcome.record.id)).expect("dict");
        assert!(dict.contains("你好\tni hao\t1"));

        let custom =
            std::fs::read_to_string(root.join("pinyin.custom.yaml")).expect("custom.yaml");
        assert!(custom.contains("  \"translator/packs/+\":\n"));
        assert!(custom.contains(&format!("    - {}\n", outcome.record.id)));

        let listed = list(root);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, outcome.record.id);
    }

    #[test]
    fn import_without_scheme_only_writes_file() {
        let dir = temp_dir();
        let root_path = rime_root(&dir);
        let root = root_path.as_path();
        let outcome = import_parsed(
            root,
            &parsed(vec![entry("你好", "ni hao")]),
            Path::new("/tmp/词库.txt"),
            &[],
        )
        .expect("import");
        assert!(outcome.dict_path.exists());
        assert!(outcome.record.attached_schemas.is_empty());
        assert!(!outcome.record.enabled);
        assert!(!root.join("pinyin.custom.yaml").exists());
    }

    #[test]
    fn import_rejects_scheme_with_wrong_translator() {
        let dir = temp_dir();
        let root_path = rime_root(&dir);
        let root = root_path.as_path();
        write_schema(root, "wubi", &["table_translator@wubi"]);
        let error = import_parsed(
            root,
            &parsed(vec![entry("你好", "ni hao")]),
            Path::new("/tmp/词库.scel"),
            &["wubi".to_string()],
        )
        .unwrap_err();
        assert!(error.message.contains("挂不了拼音词库"), "{error}");
        assert!(list(root).is_empty());
    }

    #[test]
    fn disable_and_remove_toggle_packs() {
        let dir = temp_dir();
        let root_path = rime_root(&dir);
        let root = root_path.as_path();
        write_schema(root, "pinyin", &["script_translator@translator"]);
        let outcome = import_parsed(
            root,
            &parsed(vec![entry("你好", "ni hao")]),
            Path::new("/tmp/a.scel"),
            &["pinyin".to_string()],
        )
        .expect("import");

        set_enabled(root, &outcome.record.id, false).expect("disable");
        let custom = std::fs::read_to_string(root.join("pinyin.custom.yaml")).expect("custom");
        assert!(!custom.contains(&outcome.record.id));
        assert!(!list(root)[0].enabled);

        set_enabled(root, &outcome.record.id, true).expect("enable");
        let custom = std::fs::read_to_string(root.join("pinyin.custom.yaml")).expect("custom");
        assert!(custom.contains(&outcome.record.id));

        remove(root, &outcome.record.id).expect("remove");
        assert!(list(root).is_empty());
        assert!(!dict_path(root, &outcome.record.id).exists());
        let custom = std::fs::read_to_string(root.join("pinyin.custom.yaml")).expect("custom");
        assert!(!custom.contains(&outcome.record.id));
    }

    #[test]
    fn remove_keeps_user_patch_content() {
        let dir = temp_dir();
        let root_path = rime_root(&dir);
        let root = root_path.as_path();
        write_schema(root, "pinyin", &["script_translator@translator"]);
        std::fs::write(
            root.join("pinyin.custom.yaml"),
            "patch:\n  \"menu/page_size\": 9\n",
        )
        .expect("seed");
        let outcome = import_parsed(
            root,
            &parsed(vec![entry("你好", "ni hao")]),
            Path::new("/tmp/a.scel"),
            &["pinyin".to_string()],
        )
        .expect("import");
        remove(root, &outcome.record.id).expect("remove");
        let custom = std::fs::read_to_string(root.join("pinyin.custom.yaml")).expect("custom");
        assert!(custom.contains("\"menu/page_size\": 9"));
    }

    #[test]
    fn rebind_moves_pack_between_schemes() {
        let dir = temp_dir();
        let root_path = rime_root(&dir);
        let root = root_path.as_path();
        write_schema(root, "pinyin", &["script_translator@translator"]);
        write_schema(root, "double", &["script_translator@translator"]);
        let outcome = import_parsed(
            root,
            &parsed(vec![entry("你好", "ni hao")]),
            Path::new("/tmp/a.scel"),
            &["pinyin".to_string()],
        )
        .expect("import");

        rebind(root, &outcome.record.id, &["double".to_string()]).expect("rebind");
        let old = std::fs::read_to_string(root.join("pinyin.custom.yaml")).expect("old");
        let new = std::fs::read_to_string(root.join("double.custom.yaml")).expect("new");
        assert!(!old.contains(&outcome.record.id));
        assert!(new.contains(&outcome.record.id));
        assert_eq!(list(root)[0].attached_schemas, vec!["double"]);
    }

    #[test]
    fn display_name_strips_path_and_extension() {
        assert_eq!(display_name("/a/b/网络流行语.scel"), "网络流行语");
        assert_eq!(display_name("C:\\dicts\\成语.bdict"), "成语");
        assert_eq!(display_name("wubi86.dict.yaml"), "wubi86");
        assert_eq!(display_name(".scel"), "导入词库");
    }

    /// 端到端：真实 `.scel` 样例 → 派生 dict.yaml → 方案 custom.yaml 的 packs。
    ///
    /// 样例文件借自 xime-lexicon 的 fixtures（同一个仓，避免两处各存一份二进制）。
    #[test]
    fn imports_real_scel_end_to_end() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../xime-lexicon/tests/fixtures/scel/errenzhuan.scel");
        if !sample.exists() {
            eprintln!("跳过：缺少样例 {}", sample.display());
            return;
        }
        let dir = temp_dir();
        let root_path = rime_root(&dir);
        let root = root_path.as_path();
        write_schema(root, "pinyin", &["script_translator@translator"]);

        let outcome = import_file(root, &sample, &["pinyin".to_string()]).expect("import");
        assert_eq!(outcome.record.entry_count, 2615);
        assert_eq!(outcome.record.format_id, "sogou-scel");
        assert_eq!(outcome.record.display_name, "errenzhuan");

        let dict = std::fs::read_to_string(&outcome.dict_path).expect("dict");
        assert!(dict.contains("name: "));
        assert!(dict.contains("sort: by_weight"));
        assert!(dict.contains("矮子步\tai zi bu\t1"));
        let rows = dict
            .lines()
            .filter(|line| line.contains('\t'))
            .count();
        assert_eq!(rows, 2615);

        let custom = std::fs::read_to_string(root.join("pinyin.custom.yaml")).expect("custom");
        assert!(custom.contains(&format!("- {}", outcome.record.id)));
    }

    /// 旧 `manifest.json` 一次性迁进表：方案绑定与 namespace 不能丢。
    #[test]
    fn migrates_manifest_json_into_table() {
        let dir = temp_dir();
        let root_path = rime_root(&dir);
        let root = root_path.as_path();
        let path = manifest_path(root);
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        let json = r#"{"items":[{"id":"xime_import_abc","display_name":"测试词库",
            "entry_count":42,"attached_schemas":["pinyin","wubi86"],
            "namespaces":{"pinyin":"translator","wubi86":"translator@custom"},
            "created_at":1700000000000,"format_id":"sogou-scel",
            "format_name":"搜狗细胞词库","source_name":"a.scel",
            "code_system":"Pinyin","enabled":false}]}"#;
        std::fs::write(&path, json).expect("write");

        let items = list(root);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.id, "xime_import_abc");
        assert_eq!(item.display_name, "测试词库");
        assert_eq!(item.entry_count, 42);
        assert_eq!(item.attached_schemas, vec!["pinyin", "wubi86"]);
        assert_eq!(
            item.namespaces.get("wubi86").map(String::as_str),
            Some("translator@custom")
        );
        assert!(!item.enabled);
        assert_eq!(item.format_id, "sogou-scel");
        assert_eq!(item.created_at, 1_700_000_000_000);

        // 迁完改名，别重复搬
        assert!(!path.exists());
        assert!(path.with_extension("json.migrated").exists());
        // 再读一次仍是同一条（走表，不走文件）
        assert_eq!(list(root).len(), 1);
    }

    /// `schemes` 列必须与 Android `encodeSchemes` 同格式，否则跨端读同一张表会错位。
    #[test]
    fn schemes_column_matches_android_format() {
        let mut namespaces = BTreeMap::new();
        namespaces.insert("pinyin".to_string(), "translator".to_string());
        namespaces.insert("wubi86".to_string(), "translator@custom".to_string());
        let encoded = encode_schemes(
            &["pinyin".to_string(), "wubi86".to_string()],
            &namespaces,
        );
        assert_eq!(encoded, "pinyin=translator;wubi86=translator@custom");

        let (schemas, decoded) = decode_schemes(&encoded);
        assert_eq!(schemas, vec!["pinyin", "wubi86"]);
        assert_eq!(decoded, namespaces);
        // 旧写法（只有 id）→ 默认 translator
        let (schemas, decoded) = decode_schemes("pinyin");
        assert_eq!(schemas, vec!["pinyin"]);
        assert_eq!(decoded.get("pinyin").map(String::as_str), Some("translator"));
    }

    #[test]
    fn corrupt_manifest_is_treated_as_empty() {
        let dir = temp_dir();
        let root_path = rime_root(&dir);
        let root = root_path.as_path();
        let path = manifest_path(root);
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(&path, "{not json").expect("write");
        assert!(list(root).is_empty());
    }
}
