//! 已配对设备的 SQLite 实现（`xime.db` 的 `p2p_peers` 表）。
//!
//! 协议库（`xime-p2p`）只定义读接口 [`PeerBook`]，不知道数据存在哪；
//! 存储归客户端，这里就是桌面端的实现。放 `xime.db` 而不是单独一个文件，
//! 是为了跟其它应用数据一起备份、一起迁移。
//!
//! 并发：输入法进程（读写）与设置程序（读写）会同时开着这个库，
//! SQLite 的 WAL + busy_timeout 负责跨进程互斥；进程内再用 `Mutex` 串行化。
//! 每次写都是按 `did` 的单行 `UPDATE`，不会把别台设备一起覆盖掉。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, params};
use xime_p2p::{Error as P2pError, PairedPeer, PeerBook};

/// 已配对设备的 SQLite 存储。
pub struct SqlitePeerBook {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl SqlitePeerBook {
    /// 打开（必要时创建）统一库；表由 [`crate::app_db`] 保证存在。
    pub fn open(path: impl Into<PathBuf>) -> rusqlite::Result<Self> {
        let path = path.into();
        let conn = crate::app_db::open(&path)?;
        import_legacy_peers(&conn, &path);
        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
    }

    /// 打开默认位置的统一库。
    pub fn open_default() -> rusqlite::Result<Self> {
        Self::open(crate::app_db::default_db_path())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn conn(&self) -> Result<MutexGuard<'_, Connection>, P2pError> {
        self.conn
            .lock()
            .map_err(|_| P2pError::Store("设备库锁不可用".into()))
    }
}

impl PeerBook for SqlitePeerBook {
    fn get(&self, did: &str) -> Option<PairedPeer> {
        let conn = self.conn().ok()?;
        let mut stmt = conn
            .prepare(
                "SELECT did, name, pub_ed, pub_x25519, last_addr, last_seen, paired_at \
                 FROM p2p_peers WHERE did = ?1",
            )
            .ok()?;
        stmt.query_row(params![did], row_to_peer).ok()
    }

    fn by_x25519(&self, pub_x25519: &[u8; 32]) -> Option<PairedPeer> {
        let want = hex::encode(pub_x25519);
        let conn = self.conn().ok()?;
        let mut stmt = conn
            .prepare(
                "SELECT did, name, pub_ed, pub_x25519, last_addr, last_seen, paired_at \
                 FROM p2p_peers WHERE pub_x25519 = ?1",
            )
            .ok()?;
        stmt.query_row(params![want], row_to_peer).ok()
    }

    fn all(&self) -> Vec<PairedPeer> {
        let Ok(conn) = self.conn() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT did, name, pub_ed, pub_x25519, last_addr, last_seen, paired_at \
             FROM p2p_peers ORDER BY paired_at, did",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map([], row_to_peer);
        match rows {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => Vec::new(),
        }
    }

    fn upsert(&self, peer: PairedPeer) -> Result<(), P2pError> {
        let conn = self.conn()?;
        upsert_peer(&conn, &peer).map_err(store_err)
    }

    fn remove(&self, did: &str) -> Result<bool, P2pError> {
        let conn = self.conn()?;
        let removed = conn
            .execute("DELETE FROM p2p_peers WHERE did = ?1", params![did])
            .map_err(store_err)?;
        Ok(removed > 0)
    }

    fn set_last_addr(&self, did: &str, addr: &str) -> Result<(), P2pError> {
        let conn = self.conn()?;
        let updated = conn
            .execute(
                "UPDATE p2p_peers SET last_addr = ?2 WHERE did = ?1",
                params![did, addr],
            )
            .map_err(store_err)?;
        if updated == 0 {
            return Err(P2pError::Store(format!("未找到已配对设备 {did}")));
        }
        Ok(())
    }

    fn set_last_seen(&self, did: &str, at: i64) -> Result<(), P2pError> {
        let conn = self.conn()?;
        let updated = conn
            .execute(
                "UPDATE p2p_peers SET last_seen = ?2 WHERE did = ?1",
                params![did, at],
            )
            .map_err(store_err)?;
        if updated == 0 {
            return Err(P2pError::Store(format!("未找到已配对设备 {did}")));
        }
        Ok(())
    }
}

/// 把旧版 `p2p/peers.json` 里的设备导入一次，然后把文件改名留存。
///
/// 协议库以前自己用 JSON 存设备表，迁到 `xime.db` 之后那些设备不会自动出现；
/// 不导入的话升级用户会「设备凭空消失」，得重新配对。只在表为空且文件存在时
/// 导一次，并把文件改名成 `peers.json.migrated`（不删，万一要对照）。
///
/// 失败一律忽略：导入不成功最多是少几台设备，不能挡住输入法启动。
fn import_legacy_peers(conn: &Connection, db_path: &Path) {
    let Some(dir) = db_path.parent() else {
        return;
    };
    let legacy = dir.join("p2p").join("peers.json");
    if !legacy.exists() {
        return;
    }
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM p2p_peers", [], |row| row.get(0))
        .unwrap_or(0);
    if count > 0 {
        return;
    }
    let Ok(raw) = std::fs::read_to_string(&legacy) else {
        return;
    };
    let Ok(file) = serde_json::from_str::<LegacyStoreFile>(&raw) else {
        tracing::warn!("旧配对表解析失败，跳过导入：{}", legacy.display());
        return;
    };
    let mut imported = 0usize;
    for peer in file.peers {
        if upsert_peer(conn, &peer).is_ok() {
            imported += 1;
        }
    }
    tracing::info!("已从 {} 导入 {imported} 台已配对设备", legacy.display());
    let _ = std::fs::rename(&legacy, dir.join("p2p").join("peers.json.migrated"));
}

/// 旧版 JSON 表结构（只用于迁移）。
#[derive(serde::Deserialize)]
struct LegacyStoreFile {
    #[serde(default)]
    peers: Vec<PairedPeer>,
}

/// 插入/更新一条设备（迁移与 `upsert` 共用）。
fn upsert_peer(conn: &Connection, peer: &PairedPeer) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO p2p_peers (did, name, pub_ed, pub_x25519, last_addr, last_seen, paired_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(did) DO UPDATE SET \
           name = excluded.name, \
           pub_ed = excluded.pub_ed, \
           pub_x25519 = excluded.pub_x25519, \
           paired_at = excluded.paired_at, \
           last_addr = COALESCE(p2p_peers.last_addr, excluded.last_addr), \
           last_seen = COALESCE(p2p_peers.last_seen, excluded.last_seen)",
        params![
            peer.did,
            peer.name,
            peer.pub_ed,
            peer.pub_x25519,
            peer.last_addr,
            peer.last_seen,
            peer.paired_at
        ],
    )?;
    Ok(())
}

fn row_to_peer(row: &rusqlite::Row<'_>) -> rusqlite::Result<PairedPeer> {
    Ok(PairedPeer {
        did: row.get(0)?,
        name: row.get(1)?,
        pub_ed: row.get(2)?,
        pub_x25519: row.get(3)?,
        last_addr: row.get(4)?,
        last_seen: row.get(5)?,
        paired_at: row.get(6)?,
    })
}

fn store_err(e: rusqlite::Error) -> P2pError {
    P2pError::Store(format!("设备库操作失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xime_p2p::{Identity, PublicIdentity};

    fn temp_book(name: &str) -> (PathBuf, SqlitePeerBook) {
        let dir = std::env::temp_dir().join(format!("xime_p2p_book_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("xime.db");
        let book = SqlitePeerBook::open(&path).unwrap();
        (dir, book)
    }

    fn peer(seed: u8, name: &str) -> PairedPeer {
        let id = Identity::from_seed(&[seed; 32]);
        let public: PublicIdentity = id.public(name);
        PairedPeer::from_public(&public, None)
    }

    #[test]
    fn 写入后可按_did_与公钥查回() {
        let (dir, book) = temp_book("get");
        let p = peer(1, "MacBook");
        book.upsert(p.clone()).unwrap();

        assert_eq!(book.get(&p.did).unwrap().name, "MacBook");
        let x: [u8; 32] = hex::decode(&p.pub_x25519).unwrap().try_into().unwrap();
        assert_eq!(book.by_x25519(&x).unwrap().did, p.did);
        assert_eq!(book.all().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 两个进程各开一条连接、同时改同一张表：谁都不覆盖谁。
    ///
    /// 这正是换掉 JSON 的原因——整份写回时后写的一方会用旧快照抹掉新设备。
    #[test]
    fn 两个连接写同一张表互不覆盖() {
        let (dir, a) = temp_book("concurrent");
        let path = dir.join("xime.db");
        let a_peer = peer(2, "Mac");
        a.upsert(a_peer.clone()).unwrap();
        a.set_last_addr(&a_peer.did, "192.168.1.5:47821").unwrap();

        // 另一个「进程」配对了一台新设备。
        let b = SqlitePeerBook::open(&path).unwrap();
        b.upsert(peer(3, "Phone")).unwrap();

        assert_eq!(a.all().len(), 2);
        assert_eq!(
            a.get(&a_peer.did).unwrap().last_addr.as_deref(),
            Some("192.168.1.5:47821")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 重新配对保留运行时状态() {
        let (dir, book) = temp_book("reupsert");
        let p = peer(4, "Old name");
        let did = p.did.clone();
        book.upsert(p.clone()).unwrap();
        book.set_last_addr(&did, "10.0.0.2:47821").unwrap();
        book.set_last_seen(&did, 100).unwrap();

        let mut renamed = p;
        renamed.name = "New name".to_string();
        book.upsert(renamed).unwrap();

        let got = book.get(&did).unwrap();
        assert_eq!(got.name, "New name");
        assert_eq!(got.last_addr.as_deref(), Some("10.0.0.2:47821"));
        assert_eq!(got.last_seen, Some(100));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 旧版 `p2p/peers.json` 里的设备要导入一次，然后把文件改名留存。
    /// 不导入的话，升级用户会觉得「配对的设备凭空消失了」。
    #[test]
    fn 旧版_json_设备导入一次() {
        let dir = std::env::temp_dir().join(format!("xime_p2p_mig_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("p2p")).unwrap();

        let legacy_peer = peer(7, "HUAWEI Mate 80");
        let body = serde_json::json!({ "version": 1, "peers": [legacy_peer.clone()] });
        std::fs::write(
            dir.join("p2p").join("peers.json"),
            serde_json::to_vec(&body).unwrap(),
        )
        .unwrap();

        let book = SqlitePeerBook::open(dir.join("xime.db")).unwrap();
        assert_eq!(book.all().len(), 1, "旧设备应被导入");
        assert_eq!(book.get(&legacy_peer.did).unwrap().name, "HUAWEI Mate 80");
        assert!(!dir.join("p2p").join("peers.json").exists(), "导入后应改名");
        assert!(dir.join("p2p").join("peers.json.migrated").exists());

        // 用户把设备删光后重启，不能再从备份里「复活」。
        book.remove(&legacy_peer.did).unwrap();
        drop(book);
        let reopened = SqlitePeerBook::open(dir.join("xime.db")).unwrap();
        assert!(reopened.all().is_empty(), "删除后不该被重新导入");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 删除后不再出现且未配对写地址报错() {
        let (dir, book) = temp_book("remove");
        let p = peer(5, "Old");
        let did = p.did.clone();
        book.upsert(p).unwrap();
        assert!(book.remove(&did).unwrap());
        assert!(!book.remove(&did).unwrap());
        assert!(book.set_last_addr(&did, "1.2.3.4:1").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
