//! 墓碑表（FR-45）。
//!
//! 为什么非要有这张表：只从 `message` 表删行的话，下次向上翻历史或重启后远端补齐时，
//! NapCat 会把这条消息原样送回来 —— 用户会看到"删了又出现"。
//! 所以**所有读取路径**（本地查询、远端补齐、引用解析）都必须按本表过滤。

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::Peer;
use crate::store::now_ms;

pub fn add(
    conn: &Connection,
    message_id: &str,
    peer: Peer,
    note: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO tombstone(message_id, peer_type, peer_id, deleted_at, note)
         VALUES(?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(message_id) DO NOTHING",
        params![message_id, peer.peer_type, peer.peer_id, now_ms(), note],
    )?;
    Ok(())
}

pub fn is_deleted(conn: &Connection, message_id: &str) -> Result<bool> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM tombstone WHERE message_id = ?1",
            params![message_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.is_some())
}

pub fn list_for_peer(conn: &Connection, peer: Peer) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT message_id FROM tombstone WHERE peer_type = ?1 AND peer_id = ?2
         ORDER BY deleted_at DESC",
    )?;
    let rows = stmt.query_map(params![peer.peer_type, peer.peer_id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM tombstone", [], |r| r.get(0))?)
}

/// 墓碑永久保留，但"清空某个会话的本地记录"时要把它的墓碑一起清掉，
/// 否则用户以为清空了、远端却再也不补齐（因为补齐路径按墓碑过滤）。
pub fn clear_for_peer(conn: &Connection, peer: Peer) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM tombstone WHERE peer_type = ?1 AND peer_id = ?2",
        params![peer.peer_type, peer.peer_id],
    )?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;

    #[test]
    fn 墓碑命中与未命中() {
        let db = Db::open_memory().unwrap();
        let p = Peer::group(1);
        db.tx(|c| add(c, "m1", p, Some("手滑"))).unwrap();
        assert!(db.with(|c| is_deleted(c, "m1")).unwrap());
        assert!(!db.with(|c| is_deleted(c, "m2")).unwrap());
    }

    #[test]
    fn 重复写墓碑不报错也不重复() {
        let db = Db::open_memory().unwrap();
        let p = Peer::group(1);
        db.tx(|c| {
            add(c, "m1", p, None)?;
            add(c, "m1", p, Some("再次"))?;
            Ok(())
        })
        .unwrap();
        assert_eq!(db.with(|c| count(c)).unwrap(), 1);
    }

    #[test]
    fn 墓碑是永久保留的() {
        let db = Db::open_memory().unwrap();
        let p = Peer::group(1);
        db.tx(|c| add(c, "m1", p, None)).unwrap();
        // 再来一条别的消息、以及一个会话，墓碑不受影响
        db.tx(|c| add(c, "m2", p, None)).unwrap();
        assert_eq!(db.with(|c| count(c)).unwrap(), 2);
    }

    #[test]
    fn 按会话列出被删的_id() {
        let db = Db::open_memory().unwrap();
        db.tx(|c| {
            add(c, "a", Peer::group(1), None)?;
            add(c, "b", Peer::group(1), None)?;
            add(c, "c", Peer::group(2), None)?;
            Ok(())
        })
        .unwrap();
        let got = db.with(|c| list_for_peer(c, Peer::group(1))).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.contains(&"a".to_string()));
    }

    #[test]
    fn 清空会话记录时墓碑一起清掉() {
        let db = Db::open_memory().unwrap();
        db.tx(|c| add(c, "a", Peer::group(1), None)).unwrap();
        assert_eq!(db.tx(|c| clear_for_peer(c, Peer::group(1))).unwrap(), 1);
        assert!(!db.with(|c| is_deleted(c, "a")).unwrap());
    }
}
