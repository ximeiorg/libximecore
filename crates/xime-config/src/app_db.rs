//! 统一应用数据库（`xime.db`）：一个文件装下全部「非 Rime 配置」的应用数据。
//!
//! 与 Android `XimeDatabase` 对齐到**同一 schema 版本（v7）**：
//!
//! | 表 | 用途 |
//! | --- | --- |
//! | `clipboard_entries` | 剪贴板历史 + 快捷发送（含图片元数据） |
//! | `imported_lexicons` | 外部词库导入记录（原来是 `imported_dicts/manifest.json`） |
//! | `user_ngrams` | 联想引擎的 n 元组计数（PC 端暂无写入方，建表占位） |
//! | `installed_items` | 方案包 / 方案 / 模型安装元数据（PC 端暂无写入方，建表占位） |
//!
//! **为什么统一成一个文件**：以前按功能各写各的——剪贴板进 `clipboard.db`，
//! 词库导入进 `manifest.json`，联想缓存在另一个 JSON。于是一份用户数据散在
//! 多个文件里，备份要逐个点名、损坏的恢复代价各不相同（JSON 整份覆盖写，
//! 写一半就没了）。统一后只需要搬一个文件。
//!
//! **不碰的东西**：`<方案>.custom.yaml`、`*.dict.yaml`、`custom_phrase.txt`
//! 这些是 Rime 自己要读的输入文件，不是应用存储，仍按原格式落盘。
//!
//! **跨端不搬运库文件**：PC 与 Android 各自建库，同步层只搬行数据。
//! Room 用 `room_master_table` 校验 identity hash，直接拿一边的库文件喂给
//! 另一边会被拒绝，所以这里也不写 `room_master_table`。

use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// 统一库文件名（与 Android `DATABASE_NAME` 一致）。
pub const DB_FILE: &str = "xime.db";
/// 旧版剪贴板库文件名（与 Android `LEGACY_DATABASE_NAME` 一致）。
pub const LEGACY_DB_FILE: &str = "clipboard.db";
/// schema 版本：对齐 Android `XimeDatabase.version = 7`。
pub const SCHEMA_VERSION: i64 = 7;

/// 剪贴板表（v4 全列；与 Android `ClipboardEntry` 逐列对齐）。
///
/// 只在**新库**上生效；老库缺列由 `clipboard_store::migrate_to_v4` 补齐，
/// 索引也由它建（老库缺 `imageHash` 列时这里建索引会失败）。
const CREATE_CLIPBOARD: &str = "
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

/// 导入词库记录（v5；与 Android `ImportedLexiconEntity` 逐列对齐）。
///
/// `schemes` 存方案绑定，格式 `schemaId=namespace;schemaId=namespace`
/// （与 Android `encodeSchemes` 完全一致），这样不必为少量绑定再建一张表。
const CREATE_LEXICONS: &str = "
CREATE TABLE IF NOT EXISTS imported_lexicons (
    id TEXT NOT NULL PRIMARY KEY,
    displayName TEXT NOT NULL,
    dictFile TEXT NOT NULL,
    formatId TEXT NOT NULL DEFAULT '',
    formatName TEXT NOT NULL DEFAULT '',
    codeSystem TEXT NOT NULL DEFAULT 'PINYIN',
    schemes TEXT NOT NULL DEFAULT '',
    entryCount INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    sourceName TEXT NOT NULL DEFAULT '',
    createdAt INTEGER NOT NULL DEFAULT 0
);";

/// 联想 n 元组计数（v6；PC 端暂无写入方）。
const CREATE_NGRAMS: &str = "
CREATE TABLE IF NOT EXISTS user_ngrams (
    kind INTEGER NOT NULL,
    t1 TEXT NOT NULL,
    t2 TEXT NOT NULL,
    t3 TEXT NOT NULL DEFAULT '',
    count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(kind, t1, t2, t3)
);";

/// 安装元数据（v7；PC 端暂无写入方）。
const CREATE_INSTALLED: &str = "
CREATE TABLE IF NOT EXISTS installed_items (
    kind TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    version TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(kind, id)
);";

/// 统一库的默认路径：Rime 用户目录的**同级**（`~/Library/Application Support/XimeYi/xime.db`）。
///
/// 不放进 rime 目录里：那是 Rime 自己的数据区，备份/同步会把它整体搬走，
/// 而应用数据库不属于 Rime。
pub fn default_db_path() -> PathBuf {
    let (_, user) = crate::rime_deploy::get_data_dirs();
    db_path_beside(&user)
}

/// 给定 Rime 用户目录，算出统一库路径（测试用临时目录也走这里）。
pub fn db_path_beside(user_dir: &Path) -> PathBuf {
    user_dir.parent().unwrap_or(user_dir).join(DB_FILE)
}

/// 打开统一库（必要时先把旧 `clipboard.db` 的数据搬过来），并保证表齐、版本正确。
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(path)?;
    // WAL：输入法进程（写）+ 设置程序（读）多进程同时访问同一个文件。
    conn.pragma_update(None, "journal_mode", "WAL")?;
    ensure_schema(&conn)?;
    import_legacy_rows(&conn, path);
    Ok(conn)
}

/// 打开默认位置上的统一库。
pub fn open_default() -> rusqlite::Result<Connection> {
    open(&default_db_path())
}

/// 剪贴板表的 v4 全列与缺省值（旧库缺列时按缺省值补）。
const CLIPBOARD_COLUMNS: [(&str, &str); 14] = [
    ("id", "0"),
    ("text", "''"),
    ("code", "''"),
    ("timestamp", "0"),
    ("isPinned", "0"),
    ("isQuickSend", "0"),
    ("consumed", "0"),
    ("type", "'text'"),
    ("imagePath", "''"),
    ("imageHash", "''"),
    ("mimeType", "''"),
    ("sizeBytes", "0"),
    ("width", "0"),
    ("height", "0"),
];

/// 旧 `clipboard.db` 的行搬进 `xime.db`（幂等，只在新库为空时做）。
///
/// **为什么是拷贝而不是改名**：输入法进程常驻，可能正持有 `clipboard.db`
/// 的连接（剪贴板 watcher）。`rename` 之后旧进程的 fd 仍指向老 inode，
/// 它继续写只会写进一个没人读的孤儿文件——那期间复制的东西就丢了。
/// 用 `ATTACH` 读旧库再写入新库，旧进程该怎么写还怎么写，安全得多。
///
/// 旧库可能是 v1~v4 任意版本，所以列按 `PRAGMA table_info` 动态取，
/// 缺的列补缺省值。搬成功才把旧文件改名成 `.migrated`，失败就原样留着。
fn import_legacy_rows(conn: &Connection, path: &Path) {
    let Some(dir) = path.parent() else {
        return;
    };
    let legacy = dir.join(LEGACY_DB_FILE);
    if !legacy.exists() {
        return;
    }
    // 新库已有数据：说明搬过了（或本来就在用新库），别再倒一遍。
    let existing: i64 = conn
        .query_row("SELECT COUNT(*) FROM clipboard_entries", [], |row| row.get(0))
        .unwrap_or(0);
    if existing > 0 {
        return;
    }
    if conn
        .execute("ATTACH DATABASE ?1 AS legacy", rusqlite::params![legacy.to_string_lossy()])
        .is_err()
    {
        return;
    }
    let imported = import_attached(conn);
    let _ = conn.execute_batch("DETACH DATABASE legacy");
    let Ok(imported) = imported else {
        return;
    };
    if imported == 0 {
        return;
    }
    tracing::info!(
        "[app_db] 已把 {imported} 条剪贴板记录从 {} 搬进 {}",
        LEGACY_DB_FILE,
        path.display()
    );
    // `-wal` / `-shm` 一起改名：留在原地会让后续误判「还有未 checkpoint 的旧库」。
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let source = PathBuf::from(format!("{}{suffix}", legacy.display()));
        if source.exists() {
            let _ = std::fs::rename(&source, PathBuf::from(format!("{}.migrated{suffix}", legacy.display())));
        }
    }
}

/// `ATTACH` 之后的实际导入：返回写入行数。
fn import_attached(conn: &Connection) -> rusqlite::Result<usize> {
    let mut stmt = conn.prepare("PRAGMA legacy.table_info(clipboard_entries)")?;
    let present: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<_, _>>()?;
    if present.is_empty() {
        // 旧库里没有这张表（空文件或损坏），什么都不做。
        return Ok(0);
    }
    let select: Vec<String> = CLIPBOARD_COLUMNS
        .iter()
        .map(|(name, default)| {
            if present.iter().any(|column| column == name) {
                (*name).to_string()
            } else {
                format!("{default} AS {name}")
            }
        })
        .collect();
    let columns: Vec<&str> = CLIPBOARD_COLUMNS.iter().map(|(name, _)| *name).collect();
    let sql = format!(
        "INSERT OR IGNORE INTO clipboard_entries ({}) SELECT {} FROM legacy.clipboard_entries",
        columns.join(", "),
        select.join(", ")
    );
    conn.execute(&sql, [])
}

/// 建齐四张表并把 `user_version` 抬到 [`SCHEMA_VERSION`]（幂等）。
///
/// 只做「表存在性」与版本号，不做列演进：老库缺列由各业务模块自己补
/// （剪贴板在 `clipboard_store::migrate_to_v4`），避免这里了解每张表的
/// 历史细节。
pub fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(CREATE_CLIPBOARD)?;
    conn.execute_batch(CREATE_LEXICONS)?;
    conn.execute_batch(CREATE_NGRAMS)?;
    conn.execute_batch(CREATE_INSTALLED)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < SCHEMA_VERSION {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xime_app_db_{}_{}_{}",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(DB_FILE)
    }

    #[test]
    fn creates_all_tables_and_sets_version() {
        let path = temp_db("schema");
        let conn = open(&path).unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for expected in [
            "clipboard_entries",
            "imported_lexicons",
            "installed_items",
            "user_ngrams",
        ] {
            assert!(tables.iter().any(|t| t == expected), "缺表 {expected}：{tables:?}");
        }
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 旧 `clipboard.db` 必须自动改名，且 WAL 里的数据不能丢。
    #[test]
    fn migrates_legacy_clipboard_db_file() {
        let path = temp_db("legacy");
        let dir = path.parent().unwrap().to_path_buf();
        let legacy = dir.join(LEGACY_DB_FILE);
        {
            let conn = Connection::open(&legacy).unwrap();
            conn.execute_batch(
                "CREATE TABLE clipboard_entries (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    text TEXT NOT NULL,
                    timestamp INTEGER NOT NULL DEFAULT 0,
                    isPinned INTEGER NOT NULL DEFAULT 0,
                    isQuickSend INTEGER NOT NULL DEFAULT 0
                );
                INSERT INTO clipboard_entries (text, timestamp) VALUES ('旧数据', 1);",
            )
            .unwrap();
        }

        let conn = open(&path).unwrap();
        let text: String = conn
            .query_row("SELECT text FROM clipboard_entries LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(text, "旧数据");
        assert!(path.exists(), "应改名成 xime.db");
        assert!(!legacy.exists(), "旧文件应已搬走");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧库是 v1（只有 4 列）时也要能搬：缺的列按缺省值补，不能整条丢掉。
    #[test]
    fn imports_v1_rows_with_missing_columns() {
        let path = temp_db("v1");
        let dir = path.parent().unwrap().to_path_buf();
        {
            let legacy = Connection::open(dir.join(LEGACY_DB_FILE)).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE clipboard_entries (
                        id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                        text TEXT NOT NULL,
                        timestamp INTEGER NOT NULL DEFAULT 0,
                        isPinned INTEGER NOT NULL DEFAULT 0,
                        isQuickSend INTEGER NOT NULL DEFAULT 0
                    );
                    INSERT INTO clipboard_entries (text, timestamp, isQuickSend)
                    VALUES ('老条', 100, 1);",
                )
                .unwrap();
        }

        let conn = open(&path).unwrap();
        let (text, quick, kind, consumed): (String, i64, String, i64) = conn
            .query_row(
                "SELECT text, isQuickSend, type, consumed FROM clipboard_entries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(text, "老条");
        assert_eq!(quick, 1);
        assert_eq!(kind, "text", "缺列要落默认值");
        assert_eq!(consumed, 0);

        // 再开一次不能重复导入
        drop(conn);
        let conn = open(&path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM clipboard_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 新库已经有数据时不能再从旧库倒一遍（否则删掉的条目会复活）。
    #[test]
    fn does_not_reimport_when_new_db_has_rows() {
        let path = temp_db("no_reimport");
        let dir = path.parent().unwrap().to_path_buf();
        {
            let legacy = Connection::open(dir.join(LEGACY_DB_FILE)).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE clipboard_entries (
                        id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                        text TEXT NOT NULL,
                        timestamp INTEGER NOT NULL DEFAULT 0,
                        isPinned INTEGER NOT NULL DEFAULT 0,
                        isQuickSend INTEGER NOT NULL DEFAULT 0
                    );
                    INSERT INTO clipboard_entries (text, timestamp) VALUES ('旧', 1);",
                )
                .unwrap();
        }
        {
            // 直接建新库并写入，不经过 `open()`——否则第一次打开就把旧库倒进来了。
            let conn = Connection::open(&path).unwrap();
            ensure_schema(&conn).unwrap();
            conn.execute(
                "INSERT INTO clipboard_entries (text, timestamp) VALUES ('新', 2)",
                [],
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        let texts: Vec<String> = conn
            .prepare("SELECT text FROM clipboard_entries ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(texts, vec!["新"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 重复打开不能把已有的数据或版本弄乱。
    #[test]
    fn reopen_is_idempotent() {
        let path = temp_db("reopen");
        {
            let conn = open(&path).unwrap();
            conn.execute_batch(
                "INSERT INTO imported_lexicons (id, displayName, dictFile)
                 VALUES ('xime_import_1', '测试', 'xime_import_1.dict.yaml');",
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM imported_lexicons", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
