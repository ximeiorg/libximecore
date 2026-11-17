//! 剪贴板数据存储（SQLite，与 Android clipboard.db 同构）——**全平台唯一实现**。
//!
//! 表 `clipboard_entries` 与 Android Room 实体 ClipboardEntry（v4）逐列对齐：
//! 剪贴板历史（isQuickSend=0）与快捷发送（isQuickSend=1）共用一张表；
//! 触发编码（code）、置顶（isPinned）、图片元数据列一并建齐，
//! 后续图片剪贴板 / 多端数据互通可直接复用同一 schema。
//! 旧版 JSON/YAML 文件由 [`migrate_legacy`] 幂等迁移。
//!
//! **为什么只有这一份实现**：macOS 输入法进程（写侧，NSPasteboard watcher）
//! 与设置程序（读侧，管理页）操作的是**同一个** `clipboard.db`。曾有两套独立
//! 实现（macOS 侧自建 v3 表、libximecore 侧 v4 表），写侧建表后读侧再执行
//! `CREATE INDEX ... ON clipboard_entries(imageHash)` 会因「no such column」
//! 失败，连带 `open()` 整条链返回 Err，界面表现是**剪贴板历史一片空白**
//! （而数据库里其实有上千条）。现在写侧读侧共用本模块，schema 由
//! [`migrate_to_v4`] 单点保证，任何旧版本库都能被打开并升级。
//!
//! **时间戳单位统一为毫秒**（对齐 Android `System.currentTimeMillis()`）：
//! 同一列里混用秒与毫秒会让「最新在前」的排序失真。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

/// 统一库（`xime.db`）的 schema 版本（`PRAGMA user_version`）。
///
/// 本模块的**列定义**仍对齐 Android `ClipboardEntry` v4，但整库版本由
/// [`crate::app_db`] 统一管（v7：剪贴板 + 词库记录 + n 元组 + 安装元数据）。
pub const SCHEMA_VERSION: i64 = crate::app_db::SCHEMA_VERSION;
/// 普通剪贴板条目上限（Android: `MAX_ITEMS = 1000`）。
pub const MAX_CLIPBOARD_ITEMS: i64 = 1000;
/// 快捷发送条目上限（Android: `MAX_QUICK_SEND_ITEMS = 20`）。
pub const MAX_QUICK_SEND_ITEMS: i64 = 20;

/// Android ClipboardEntry v4 建表语句（表名/列名/默认值逐列对齐）。
const CREATE_TABLE: &str = "
CREATE TABLE IF NOT EXISTS clipboard_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,
    code TEXT NOT NULL DEFAULT '',
    timestamp INTEGER NOT NULL DEFAULT 0,
    isPinned INTEGER NOT NULL DEFAULT 0,
    isQuickSend INTEGER NOT NULL DEFAULT 0,
    consumed INTEGER NOT NULL DEFAULT 0,
    type TEXT NOT NULL DEFAULT 'text',
    imagePath TEXT NOT NULL DEFAULT '',
    imageHash TEXT NOT NULL DEFAULT '',
    mimeType TEXT NOT NULL DEFAULT '',
    sizeBytes INTEGER NOT NULL DEFAULT 0,
    width INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0
);";

const CREATE_INDEX_TEXT: &str =
    "CREATE INDEX IF NOT EXISTS index_clipboard_entries_text ON clipboard_entries(text);";
const CREATE_INDEX_IMAGE_HASH: &str =
    "CREATE INDEX IF NOT EXISTS index_clipboard_entries_imageHash ON clipboard_entries(imageHash);";

/// 查询用的列清单（v4 全列）。
///
/// 必须与 [`row_to_entry`] 读取的字段一一对应：少选一列，`row.get()` 会以
/// `InvalidColumnName` 失败——而这类错误在 `unwrap_or_default()` 下会伪装成
/// 「列表为空」，极难定位。
const SELECT_COLUMNS: &str = "id, text, code, timestamp, isPinned, isQuickSend, consumed, \
     type, imagePath, imageHash, mimeType, sizeBytes, width, height";

/// 旧版本库需要补齐的列（列名 → 列定义）。
///
/// 只收录「允许缺失且补齐安全」的列：`ALTER TABLE ADD COLUMN` 不能添加
/// NOT NULL 且无默认值的列（表非空时报 "Cannot add a NOT NULL column"），
/// 而 `text` / `timestamp` / `isPinned` / `isQuickSend` 从 v1 起就存在，
/// 无需补齐。列名反引号包住：`type` 在部分 SQL 方言里是关键字。
const MIGRATABLE_COLUMNS: [(&str, &str); 9] = [
    ("consumed", "INTEGER NOT NULL DEFAULT 0"),
    ("code", "TEXT NOT NULL DEFAULT ''"),
    ("type", "TEXT NOT NULL DEFAULT 'text'"),
    ("imagePath", "TEXT NOT NULL DEFAULT ''"),
    ("imageHash", "TEXT NOT NULL DEFAULT ''"),
    ("mimeType", "TEXT NOT NULL DEFAULT ''"),
    ("sizeBytes", "INTEGER NOT NULL DEFAULT 0"),
    ("width", "INTEGER NOT NULL DEFAULT 0"),
    ("height", "INTEGER NOT NULL DEFAULT 0"),
];

/// 默认数据库路径：用户数据目录同级 `clipboard.db`（同 Android 库名）。
///
/// 这是**唯一的路径来源**：输入法进程与设置程序都经由 `get_data_dirs()` 解析
/// 用户数据目录，再取其同级 `clipboard.db`，从而保证读写落在同一个文件上。
pub fn default_db_path() -> PathBuf {
    crate::app_db::default_db_path()
}

/// 当前时间（Unix 毫秒，对齐 Android `System.currentTimeMillis()`）。
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 单条剪贴板/快捷发送条目（对应 Android `ClipboardEntry`）。
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct ClipboardEntry {
    pub id: i64,
    pub text: String,
    /// 快捷发送触发编码（如 "dh"），普通剪贴板条目为空串。
    pub code: String,
    /// 时间戳（Unix 毫秒）。
    pub timestamp: i64,
    /// 置顶：置顶条目永不被自动裁剪。
    pub is_pinned: bool,
    /// false = 普通剪贴板条目；true = 快捷发送条目。
    pub is_quick_send: bool,
    /// 候选栏「已消费」标记（Android 语义，面板上屏时置位）。
    pub consumed: bool,
    /// 条目类型（`text` / `image` 等，对应 `type` 列）。
    pub entry_type: String,
    /// 图片本地路径（图片剪贴板用）。
    pub image_path: String,
    /// 图片内容哈希（去重与多端比对用）。
    pub image_hash: String,
    /// 图片 MIME 类型。
    pub mime_type: String,
    /// 图片字节数。
    pub size_bytes: i64,
    /// 图片宽（像素）。
    pub width: i64,
    /// 图片高（像素）。
    pub height: i64,
}

impl ClipboardEntry {
    /// 是否为快捷发送条目（与 Android 相同命名）。
    pub fn is_quick_send(&self) -> bool {
        self.is_quick_send
    }
}

/// 剪贴板存储（持有一条连接，`Mutex` 保证 Send+Sync）。
///
/// 手写 SQL（无 ORM），与 `xime-sync-store` 的 `HistoryRepo` 风格一致。
/// 输入法进程用它做写侧高频操作（每 500ms 轮询一次），因此连接复用而非
/// 每次开关。
pub struct ClipboardStore {
    conn: Mutex<Connection>,
}

impl ClipboardStore {
    /// 打开（或创建）数据库并把 schema 升级到 v4。
    pub fn open(db_path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        // 库文件、WAL、版本号统一由 app_db 负责（含旧 clipboard.db 改名），
        // 这里只补自己的列——剪贴板只是 xime.db 里的一张表。
        let conn = crate::app_db::open(db_path.as_ref())?;
        migrate_to_v4(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 内存库（降级用）：磁盘库打不开时提供空实例，避免调用方 panic。
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        crate::app_db::ensure_schema(&conn)?;
        migrate_to_v4(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 最近普通剪贴板条目（timestamp 倒序，最多 `limit` 条）。
    pub fn list_clipboard(&self, limit: i64) -> rusqlite::Result<Vec<ClipboardEntry>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM clipboard_entries
             WHERE isQuickSend = 0
             ORDER BY timestamp DESC, id DESC
             LIMIT ?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit], row_to_entry)?;
        rows.collect()
    }

    /// 全部快捷发送条目（timestamp 倒序）。
    pub fn list_quick_send(&self) -> rusqlite::Result<Vec<ClipboardEntry>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM clipboard_entries
             WHERE isQuickSend = 1
             ORDER BY timestamp DESC, id DESC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_entry)?;
        rows.collect()
    }

    /// 按 id 查找条目。
    pub fn find_by_id(&self, id: i64) -> rusqlite::Result<Option<ClipboardEntry>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("SELECT {SELECT_COLUMNS} FROM clipboard_entries WHERE id = ?1 LIMIT 1");
        conn.query_row(&sql, params![id], row_to_entry).optional()
    }

    /// 普通剪贴板：按 text 去重插入/刷新，超出上限裁剪最旧未置顶条目
    /// （对应 Android `upsertAndTrim`）。
    pub fn upsert_and_trim(&self, text: &str, now: i64) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let existing = find_by_text(&tx, text, false)?;
        if let Some(id) = existing {
            update_timestamp(&tx, id, now)?;
        } else {
            tx.execute(
                "INSERT INTO clipboard_entries (text, code, timestamp, isPinned, isQuickSend, consumed)
                 VALUES (?1, '', ?2, 0, 0, 0)",
                params![text, now],
            )?;
            trim_unpinned(&tx)?;
        }
        tx.commit()
    }

    /// 把普通剪贴板条目加入快捷发送（复制为快捷发送条目，默认置顶；
    /// 对应 Android `addQuickSend`）。
    pub fn add_to_quick_send(&self, source_id: i64, now: i64) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let source: Option<String> = tx
            .query_row(
                "SELECT text FROM clipboard_entries WHERE id = ?1 AND isQuickSend = 0",
                params![source_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(text) = source else {
            return Ok(());
        };
        if let Some(id) = find_by_text(&tx, &text, true)? {
            update_timestamp(&tx, id, now)?;
        } else {
            tx.execute(
                "INSERT INTO clipboard_entries (text, code, timestamp, isPinned, isQuickSend, consumed)
                 VALUES (?1, '', ?2, 1, 1, 0)",
                params![&text, now],
            )?;
            trim_quick_send(&tx)?;
        }
        tx.commit()
    }

    /// 新建/更新快捷发送条目（手填文本 + 触发编码；对应 Android `insertQuickSend`）。
    pub fn insert_quick_send(&self, text: &str, code: &str, now: i64) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        if let Some(id) = find_by_text(&tx, text, true)? {
            tx.execute(
                "UPDATE clipboard_entries SET text = ?1, code = ?2, timestamp = ?3 WHERE id = ?4",
                params![text, code, now, id],
            )?;
        } else {
            tx.execute(
                "INSERT INTO clipboard_entries (text, code, timestamp, isPinned, isQuickSend, consumed)
                 VALUES (?1, ?2, ?3, 1, 1, 0)",
                params![text, code, now],
            )?;
            trim_quick_send(&tx)?;
        }
        tx.commit()
    }

    /// 更新快捷发送条目的文本与编码（对应 Android `updateQuickSendItem`）。
    pub fn update_quick_send_item(
        &self,
        id: i64,
        text: &str,
        code: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE clipboard_entries SET text = ?1, code = ?2, timestamp = ?3 WHERE id = ?4 AND isQuickSend = 1",
            params![text, code, now, id],
        )?;
        Ok(())
    }

    /// 刷新条目时间戳（置顶语义：时间靠前 = 排在列表最前）。
    pub fn update_timestamp(&self, id: i64, now: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        update_timestamp(&conn, id, now)
    }

    /// 标记条目已消费（对应 Android `markConsumed`）。
    pub fn mark_consumed(&self, id: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE clipboard_entries SET consumed = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// 删除普通剪贴板条目（不影响快捷发送）。
    pub fn delete_clipboard(&self, id: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM clipboard_entries WHERE isQuickSend = 0 AND id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// 删除快捷发送条目（不影响普通剪贴板）。
    pub fn delete_quick_send(&self, id: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM clipboard_entries WHERE isQuickSend = 1 AND id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// 清空普通剪贴板（保留快捷发送）。
    pub fn clear_clipboard(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM clipboard_entries WHERE isQuickSend = 0", [])?;
        Ok(())
    }

    /// 清空快捷发送（保留普通剪贴板）。
    pub fn clear_quick_send(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM clipboard_entries WHERE isQuickSend = 1", [])?;
        Ok(())
    }

    /// 清空全部条目。
    pub fn clear_all(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM clipboard_entries", [])?;
        Ok(())
    }

    /// 普通剪贴板条目总数。
    pub fn count_clipboard(&self) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM clipboard_entries WHERE isQuickSend = 0",
            [],
            |r| r.get(0),
        )
    }

    /// 快捷发送条目总数。
    pub fn count_quick_send(&self) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM clipboard_entries WHERE isQuickSend = 1",
            [],
            |r| r.get(0),
        )
    }
}

/// 把已打开的库升级到 v4（幂等）。
///
/// - 表不存在 → 按 v4 建表；
/// - 表存在但缺列（v1/v2/v3 旧库）→ 逐列 `ALTER TABLE ADD COLUMN` 补齐；
/// - **补列在建索引之前**：列补齐后 `imageHash` 索引才建得出来，否则
///   `CREATE INDEX` 会因「no such column」失败，把整条 `open()` 链打断。
///
/// 这是本次「剪贴板历史空白」事故的通用兜底：无论库里是哪个版本的 schema，
/// 打开后都是可读的 v4。
fn migrate_to_v4(conn: &Connection) -> rusqlite::Result<()> {
    let table_exists: bool = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'clipboard_entries'",
        [],
        |r| r.get::<_, i64>(0),
    )? > 0;
    if !table_exists {
        conn.execute_batch(CREATE_TABLE)?;
    } else {
        for (name, definition) in MIGRATABLE_COLUMNS {
            if !column_exists(conn, name)? {
                conn.execute_batch(&format!(
                    "ALTER TABLE clipboard_entries ADD COLUMN `{name}` {definition}"
                ))?;
            }
        }
    }
    conn.execute_batch(CREATE_INDEX_TEXT)?;
    conn.execute_batch(CREATE_INDEX_IMAGE_HASH)?;
    // 版本号不在这里写：整库版本归 app_db 管（这里落成 4 会把 v7 打回去）。
    Ok(())
}

/// 表里是否已有该列。
fn column_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(clipboard_entries)")?;
    let columns = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for column in columns {
        if column? == name {
            return Ok(true);
        }
    }
    Ok(false)
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<ClipboardEntry> {
    Ok(ClipboardEntry {
        id: row.get("id")?,
        text: row.get("text")?,
        code: row.get("code")?,
        timestamp: row.get("timestamp")?,
        is_pinned: row.get::<_, i64>("isPinned")? != 0,
        is_quick_send: row.get::<_, i64>("isQuickSend")? != 0,
        consumed: row.get::<_, i64>("consumed")? != 0,
        entry_type: row.get("type")?,
        image_path: row.get("imagePath")?,
        image_hash: row.get("imageHash")?,
        mime_type: row.get("mimeType")?,
        size_bytes: row.get("sizeBytes")?,
        width: row.get("width")?,
        height: row.get("height")?,
    })
}

fn find_by_text(
    conn: &Connection,
    text: &str,
    is_quick_send: bool,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM clipboard_entries
         WHERE text = ?1 AND isQuickSend = ?2 LIMIT 1",
        params![text, is_quick_send as i64],
        |r| r.get(0),
    )
    .optional()
}

fn update_timestamp(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE clipboard_entries SET timestamp = ?1 WHERE id = ?2",
        params![now, id],
    )?;
    Ok(())
}

/// 裁剪最旧的未置顶条目（置顶条目永不被自动裁剪；对应 Android `trimUnpinned`）。
fn trim_unpinned(tx: &Transaction) -> rusqlite::Result<()> {
    let unpinned: i64 = tx.query_row(
        "SELECT COUNT(*) FROM clipboard_entries WHERE isPinned = 0",
        [],
        |r| r.get(0),
    )?;
    if unpinned > MAX_CLIPBOARD_ITEMS {
        tx.execute(
            "DELETE FROM clipboard_entries
             WHERE isPinned = 0
             AND id IN (
                 SELECT id FROM clipboard_entries
                 WHERE isPinned = 0 ORDER BY timestamp ASC LIMIT ?1
             )",
            params![unpinned - MAX_CLIPBOARD_ITEMS],
        )?;
    }
    Ok(())
}

/// 裁剪最旧的快捷发送条目（同称 Android `trimQuickSend`）。
fn trim_quick_send(tx: &Transaction) -> rusqlite::Result<()> {
    let count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM clipboard_entries WHERE isQuickSend = 1",
        [],
        |r| r.get(0),
    )?;
    if count > MAX_QUICK_SEND_ITEMS {
        tx.execute(
            "DELETE FROM clipboard_entries
             WHERE isQuickSend = 1
             AND id IN (
                 SELECT id FROM clipboard_entries
                 WHERE isQuickSend = 1 ORDER BY timestamp ASC LIMIT ?1
             )",
            params![count - MAX_QUICK_SEND_ITEMS],
        )?;
    }
    Ok(())
}

// ------------------------------------------------------------------
// 自由函数 API（多进程场景：server / 设置程序各自开连接，不共享状态）
// ------------------------------------------------------------------

/// 追加一条剪贴板历史：同文本去重（旧条目删除、新条目置顶），超出容量裁剪。
pub fn append_history(path: &Path, text: &str, cap: usize) -> rusqlite::Result<()> {
    let conn = open(path)?;
    conn.execute(
        "DELETE FROM clipboard_entries WHERE text = ?1 AND isQuickSend = 0",
        [text],
    )?;
    conn.execute(
        "INSERT INTO clipboard_entries (text, code, timestamp, isQuickSend) VALUES (?1, '', ?2, 0)",
        rusqlite::params![text, now_millis()],
    )?;
    conn.execute(
        "DELETE FROM clipboard_entries WHERE id IN (
            SELECT id FROM clipboard_entries WHERE isQuickSend = 0
            ORDER BY timestamp DESC, id DESC LIMIT -1 OFFSET ?1
        )",
        [cap as i64],
    )?;
    Ok(())
}

/// 列出剪贴板历史（最新在前）。
pub fn list_history(path: &Path, limit: usize) -> rusqlite::Result<Vec<ClipboardEntry>> {
    let conn = open(path)?;
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM clipboard_entries
         WHERE isQuickSend = 0 ORDER BY timestamp DESC, id DESC LIMIT ?1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([limit as i64], row_to_entry)?;
    rows.collect()
}

/// 删除单条历史（按 id）。
pub fn remove_history_item(path: &Path, id: i64) -> rusqlite::Result<()> {
    let conn = open(path)?;
    conn.execute(
        "DELETE FROM clipboard_entries WHERE id = ?1 AND isQuickSend = 0",
        [id],
    )?;
    Ok(())
}

/// 清空剪贴板历史（保留快捷发送条目）。
pub fn clear_history(path: &Path) -> rusqlite::Result<()> {
    let conn = open(path)?;
    conn.execute("DELETE FROM clipboard_entries WHERE isQuickSend = 0", [])?;
    Ok(())
}

/// 列出快捷发送条目（置顶优先，其次时间倒序）。
pub fn list_quick_send(path: &Path) -> rusqlite::Result<Vec<ClipboardEntry>> {
    let conn = open(path)?;
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM clipboard_entries
         WHERE isQuickSend = 1 ORDER BY isPinned DESC, timestamp DESC, id DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_entry)?;
    rows.collect()
}

/// 添加快捷发送条目（同文本去重）。
pub fn add_quick_send(path: &Path, text: &str, code: &str) -> rusqlite::Result<()> {
    let conn = open(path)?;
    conn.execute(
        "DELETE FROM clipboard_entries WHERE text = ?1 AND isQuickSend = 1",
        [text],
    )?;
    conn.execute(
        "INSERT INTO clipboard_entries (text, code, timestamp, isQuickSend) VALUES (?1, ?2, ?3, 1)",
        rusqlite::params![text, code, now_millis()],
    )?;
    Ok(())
}

/// 删除快捷发送条目。
pub fn remove_quick_send(path: &Path, id: i64) -> rusqlite::Result<()> {
    let conn = open(path)?;
    conn.execute(
        "DELETE FROM clipboard_entries WHERE id = ?1 AND isQuickSend = 1",
        [id],
    )?;
    Ok(())
}

/// 打开连接并确保表结构为 v4（WAL，多进程读写安全）。
fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate_to_v4(&conn)?;
    Ok(conn)
}

/// 迁移旧版 JSON/YAML 文件（幂等：迁移后改名 *.migrated）。
pub fn migrate_legacy(db: &Path, legacy_history_json: &Path, legacy_quick_send_yaml: &Path) {
    let _ = std::fs::create_dir_all(db.parent().unwrap_or(Path::new(".")));
    if legacy_history_json.exists() {
        if let Ok(content) = std::fs::read_to_string(legacy_history_json) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
                    if let Ok(conn) = open(db) {
                        // 文件最新在前 → 倒序插入，迁移后顺序不变
                        for item in items.iter().rev() {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                let ts = item
                                    .get("timestamp")
                                    .and_then(|t| t.as_i64())
                                    .unwrap_or_else(now_millis);
                                let _ = conn.execute(
                                    "DELETE FROM clipboard_entries WHERE text = ?1 AND isQuickSend = 0",
                                    [text],
                                );
                                let _ = conn.execute(
                                    "INSERT INTO clipboard_entries (text, code, timestamp, isQuickSend)
                                     VALUES (?1, '', ?2, 0)",
                                    rusqlite::params![text, ts],
                                );
                            }
                        }
                    }
                }
            }
        }
        let _ = std::fs::rename(
            legacy_history_json,
            legacy_history_json.with_extension("json.migrated"),
        );
    }
    if legacy_quick_send_yaml.exists() {
        if let Ok(content) = std::fs::read_to_string(legacy_quick_send_yaml) {
            if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
                if let Some(items) = value.get("items").and_then(|v| v.as_sequence()) {
                    if let Ok(conn) = open(db) {
                        for item in items {
                            if let Some(text) = item.get("content").and_then(|t| t.as_str()) {
                                let _ = conn.execute(
                                    "DELETE FROM clipboard_entries WHERE text = ?1 AND isQuickSend = 1",
                                    [text],
                                );
                                let _ = conn.execute(
                                    "INSERT INTO clipboard_entries (text, code, timestamp, isQuickSend)
                                     VALUES (?1, '', ?2, 1)",
                                    rusqlite::params![text, now_millis()],
                                );
                            }
                        }
                    }
                }
            }
        }
        let _ = std::fs::rename(
            legacy_quick_send_yaml,
            legacy_quick_send_yaml.with_extension("yaml.migrated"),
        );
    }
}

/// 便捷别名：进程内共享一把剪贴板存储（输入法进程用）。
pub type SharedClipboardStore = Arc<ClipboardStore>;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xime_clip_store_{label}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        dir.join("clipboard.db")
    }

    #[test]
    fn history_dedup_moves_to_front_and_caps() {
        let db = temp_db("dedup");
        append_history(&db, "第一条", 50).unwrap();
        append_history(&db, "第二条", 50).unwrap();
        append_history(&db, "第一条", 50).unwrap();
        let items = list_history(&db, 50).unwrap();
        assert_eq!(items.len(), 2, "重复内容不产生重复条目");
        assert_eq!(items[0].text, "第一条", "重新复制移到最前");
        // 容量裁剪：快捷发送条目不受影响
        add_quick_send(&db, "置顶短语", "").unwrap();
        for i in 0..60 {
            append_history(&db, &format!("内容{i}"), 50).unwrap();
        }
        let items = list_history(&db, 100).unwrap();
        assert!(items.len() <= 50, "历史裁剪到容量上限");
        let quick = list_quick_send(&db).unwrap();
        assert_eq!(quick.len(), 1, "快捷发送不被历史裁剪波及");
    }

    #[test]
    fn quick_send_add_remove_and_history_clear_keeps_quick_send() {
        let db = temp_db("quick");
        add_quick_send(&db, "您好，请问", "dh").unwrap();
        add_quick_send(&db, "收到", "").unwrap();
        add_quick_send(&db, "您好，请问", "dh").unwrap(); // 去重
        let items = list_quick_send(&db).unwrap();
        assert_eq!(items.len(), 2, "收到 + 重加的您好（同文本去重后不重复）");
        assert_eq!(items[0].code, "dh", "重加的您好排最前");
        assert_eq!(items[1].text, "收到");
        append_history(&db, "临时内容", 50).unwrap();
        clear_history(&db).unwrap();
        assert!(list_history(&db, 50).unwrap().is_empty());
        assert_eq!(list_quick_send(&db).unwrap().len(), 2, "清空历史保留快捷发送");
        remove_quick_send(&db, items[0].id).unwrap();
        assert_eq!(list_quick_send(&db).unwrap().len(), 1);
    }

    #[test]
    fn legacy_files_migrate_into_db() {
        let dir = temp_db("legacy").parent().unwrap().to_path_buf();
        let db = dir.join("clipboard.db");
        let json = dir.join("clipboard_history.json");
        let yaml = dir.join("quick_send.yaml");
        std::fs::write(
            &json,
            r#"{"items":[{"text":"旧历史","timestamp":100},{"text":"最新","timestamp":200}]}"#,
        )
        .unwrap_or_default();
        std::fs::write(&yaml, "items:\n  - name: 称呼\n    content: 您好\n").unwrap_or_default();
        migrate_legacy(&db, &json, &yaml);
        let items = list_history(&db, 50).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "最新", "迁移保持最新在前");
        let quick = list_quick_send(&db).unwrap();
        assert_eq!(quick.len(), 1);
        assert_eq!(quick[0].text, "您好");
        assert!(!json.exists(), "迁移后改名");
        assert!(!yaml.exists());
    }

    /// 回归：macOS 侧曾自建 v3 表（7 列，缺 imageHash），读侧 open() 执行
    /// `CREATE INDEX ... imageHash` 报「no such column」→ 整条链 Err →
    /// 设置程序剪贴板历史一片空白。这里验证旧库能被打开并升级到 v4。
    #[test]
    fn opens_legacy_v3_db_and_upgrades_to_v4() {
        let db = temp_db("legacy_v3");
        {
            let conn = Connection::open(&db).unwrap();
            // macOS 输入法进程（ximeyi-clipboard v3）建出的表结构。
            conn.execute_batch(
                "CREATE TABLE clipboard_entries (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                    text        TEXT NOT NULL,
                    timestamp   INTEGER NOT NULL DEFAULT 0,
                    isPinned    INTEGER NOT NULL DEFAULT 0,
                    isQuickSend INTEGER NOT NULL DEFAULT 0,
                    consumed    INTEGER NOT NULL DEFAULT 0,
                    code        TEXT NOT NULL DEFAULT ''
                );
                CREATE INDEX index_clipboard_entries_text ON clipboard_entries(text);
                INSERT INTO clipboard_entries (text, timestamp) VALUES ('旧数据', 100);",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 3).unwrap();
        }

        // 打开即升级（此前这一步会因 imageHash 索引失败）。
        let store = ClipboardStore::open(&db).unwrap();
        let items = store.list_clipboard(10).unwrap();
        assert_eq!(items.len(), 1, "旧库数据仍在");
        assert_eq!(items[0].text, "旧数据");

        // 升级后应有 v4 全部列。
        let conn = store.conn.lock().unwrap();
        for col in [
            "consumed",
            "code",
            "type",
            "imagePath",
            "imageHash",
            "mimeType",
            "sizeBytes",
            "width",
            "height",
        ] {
            assert!(column_exists(&conn, col).unwrap(), "v4 缺少列 {col}");
        }
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    /// 剪贴板已并入统一库：路径必须指向 `xime.db`，不是旧的 `clipboard.db`。
    #[test]
    fn default_path_is_unified_db() {
        let path = default_db_path();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(crate::app_db::DB_FILE)
        );
    }

    /// 写侧（IME 进程）与读侧（设置程序）必须落在同一 schema 上：
    /// 写侧建出的 v4 库，读侧能直接打开并读到数据。
    #[test]
    fn writer_and_reader_share_one_db() {
        let db = temp_db("shared");
        let writer = ClipboardStore::open(&db).unwrap();
        writer.upsert_and_trim("https://arxiv.org/abs/2610.14162", now_millis()).unwrap();
        writer.upsert_and_trim("https://arxiv.org/abs/2610.14162", now_millis()).unwrap();

        let reader = ClipboardStore::open(&db).unwrap();
        let items = reader.list_clipboard(50).unwrap();
        assert_eq!(items.len(), 1, "同文本去重");
        assert_eq!(items[0].text, "https://arxiv.org/abs/2610.14162");
    }

    /// 时间戳统一为毫秒，否则与 Android 互通后排序失真。
    #[test]
    fn timestamps_are_milliseconds() {
        let now = now_millis();
        assert!(now > 1_600_000_000_000, "应为毫秒级时间戳，实际 {now}");
        let db = temp_db("millis");
        append_history(&db, "条目", 50).unwrap();
        let items = list_history(&db, 10).unwrap();
        assert!(items[0].timestamp > 1_600_000_000_000);
    }
}
