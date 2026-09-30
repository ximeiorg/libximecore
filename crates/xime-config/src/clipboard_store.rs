//! 剪贴板数据存储（SQLite，与 Android clipboard.db 同构）。
//!
//! 表 `clipboard_entries` 与 Android Room 实体 ClipboardEntry（v4）逐列对齐：
//! 剪贴板历史（isQuickSend=0）与快捷发送（isQuickSend=1）共用一张表；
//! 触发编码（code）、置顶（isPinned）、图片元数据列一并建齐，
//! 后续图片剪贴板 / 多端数据互通可直接复用同一 schema。
//! 旧版 JSON/YAML 文件由 [`migrate_legacy`] 幂等迁移。

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// 默认数据库路径：用户数据目录同级 `clipboard.db`（同 Android 库名）。
pub fn default_db_path() -> PathBuf {
    let (_, user) = crate::rime_deploy::get_data_dirs();
    user.parent().unwrap_or(&user).join("clipboard.db")
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 打开连接并确保表结构（WAL，server / 设置程序多进程读写安全）。
fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(CREATE_TABLE)?;
    conn.execute_batch(CREATE_INDEX_TEXT)?;
    conn.execute_batch(CREATE_INDEX_IMAGE_HASH)?;
    Ok(conn)
}

/// 历史条目（isQuickSend=0 子集）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipboardHistoryItem {
    pub id: i64,
    pub text: String,
    pub timestamp: i64,
}

/// 快捷发送条目（isQuickSend=1 子集，对齐 Android QuickSendItem）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct QuickSendItem {
    pub id: i64,
    pub text: String,
    pub code: String,
    pub timestamp: i64,
    pub is_pinned: bool,
}

/// 追加一条剪贴板历史：同文本去重（旧条目删除、新条目置顶），超出容量裁剪。
pub fn append_history(path: &Path, text: &str, cap: usize) -> rusqlite::Result<()> {
    let conn = open(path)?;
    conn.execute(
        "DELETE FROM clipboard_entries WHERE text = ?1 AND isQuickSend = 0",
        [text],
    )?;
    conn.execute(
        "INSERT INTO clipboard_entries (text, code, timestamp, isQuickSend) VALUES (?1, '', ?2, 0)",
        rusqlite::params![text, now_secs()],
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
pub fn list_history(path: &Path, limit: usize) -> rusqlite::Result<Vec<ClipboardHistoryItem>> {
    let conn = open(path)?;
    let mut stmt = conn.prepare(
        "SELECT id, text, timestamp FROM clipboard_entries
         WHERE isQuickSend = 0 ORDER BY timestamp DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(ClipboardHistoryItem {
            id: row.get(0)?,
            text: row.get(1)?,
            timestamp: row.get(2)?,
        })
    })?;
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

/// 列出快捷发送条目（新在前，置顶优先）。
pub fn list_quick_send(path: &Path) -> rusqlite::Result<Vec<QuickSendItem>> {
    let conn = open(path)?;
    let mut stmt = conn.prepare(
        "SELECT id, text, code, timestamp, isPinned FROM clipboard_entries
         WHERE isQuickSend = 1 ORDER BY isPinned DESC, timestamp DESC, id DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(QuickSendItem {
            id: row.get(0)?,
            text: row.get(1)?,
            code: row.get(2)?,
            timestamp: row.get(3)?,
            is_pinned: row.get::<_, i64>(4)? != 0,
        })
    })?;
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
        rusqlite::params![text, code, now_secs()],
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
                                    .unwrap_or_else(now_secs);
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
                                    rusqlite::params![text, now_secs()],
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xime_clip_store_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        dir.join("clipboard.db")
    }

    #[test]
    fn history_dedup_moves_to_front_and_caps() {
        let db = temp_db("dedup");
        append_history(&db, "第一条", 50).unwrap_or_default();
        append_history(&db, "第二条", 50).unwrap_or_default();
        append_history(&db, "第一条", 50).unwrap_or_default();
        let items = list_history(&db, 50).unwrap_or_default();
        assert_eq!(items.len(), 2, "重复内容不产生重复条目");
        assert_eq!(items[0].text, "第一条", "重新复制移到最前");
        // 容量裁剪：快捷发送条目不受影响
        add_quick_send(&db, "置顶短语", "").unwrap_or_default();
        for i in 0..60 {
            append_history(&db, &format!("内容{i}"), 50).unwrap_or_default();
        }
        let items = list_history(&db, 100).unwrap_or_default();
        assert!(items.len() <= 50, "历史裁剪到容量上限");
        let quick = list_quick_send(&db).unwrap_or_default();
        assert_eq!(quick.len(), 1, "快捷发送不被历史裁剪波及");
    }

    #[test]
    fn quick_send_add_remove_and_history_clear_keeps_quick_send() {
        let db = temp_db("quick");
        add_quick_send(&db, "您好，请问", "dh").unwrap_or_default();
        add_quick_send(&db, "收到", "").unwrap_or_default();
        add_quick_send(&db, "您好，请问", "dh").unwrap_or_default(); // 去重
        let items = list_quick_send(&db).unwrap_or_default();
        assert_eq!(items.len(), 2, "收到 + 重加的您好（同文本去重后不重复）");
        assert_eq!(items[0].code, "dh", "重加的您好排最前");
        assert_eq!(items[1].text, "收到");
        append_history(&db, "临时内容", 50).unwrap_or_default();
        clear_history(&db).unwrap_or_default();
        assert!(list_history(&db, 50).unwrap_or_default().is_empty());
        assert_eq!(
            list_quick_send(&db).unwrap_or_default().len(),
            2,
            "清空历史保留快捷发送"
        );
        remove_quick_send(&db, items[0].id).unwrap_or_default();
        assert_eq!(list_quick_send(&db).unwrap_or_default().len(), 1);
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
        let items = list_history(&db, 50).unwrap_or_default();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "最新", "迁移保持最新在前");
        let quick = list_quick_send(&db).unwrap_or_default();
        assert_eq!(quick.len(), 1);
        assert_eq!(quick[0].text, "您好");
        assert!(!json.exists(), "迁移后改名");
        assert!(!yaml.exists());
    }
}
