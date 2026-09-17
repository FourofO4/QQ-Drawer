//! 本地静音表（FR-36 / FR-37）。
//!
//! **本表只影响本机的提醒行为，不调用任何 NapCat 接口。**
//! 这是产品的一条硬承诺：你在手机上给谁静音，跟这里毫无关系，反之亦然。
//! 技术上也不可能写进 QQ —— OneBot 事件里没有免打扰字段，NapCat 也没有对应接口（§6.1）。

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::model::Peer;
use crate::store::now_ms;

pub fn set(conn: &Connection, peer: Peer, muted: bool, reason: Option<&str>) -> Result<()> {
    if muted {
        conn.execute(
            "INSERT INTO mute(peer_type, peer_id, muted_at, reason) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(peer_type, peer_id) DO UPDATE SET muted_at = excluded.muted_at",
            params![peer.peer_type, peer.peer_id, now_ms(), reason],
        )?;
    } else {
        conn.execute(
            "DELETE FROM mute WHERE peer_type = ?1 AND peer_id = ?2",
            params![peer.peer_type, peer.peer_id],
        )?;
    }
    Ok(())
}

pub fn is_muted(conn: &Connection, peer: Peer) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM mute WHERE peer_type = ?1 AND peer_id = ?2",
        params![peer.peer_type, peer.peer_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn list(conn: &Connection) -> Result<Vec<Peer>> {
    let mut stmt = conn.prepare("SELECT peer_type, peer_id FROM mute ORDER BY muted_at DESC")?;
    let rows = stmt.query_map([], |r| {
        Ok(Peer { peer_type: r.get(0)?, peer_id: r.get(1)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM mute", [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{conversation, Db};

    fn db() -> Db {
        let db = Db::open_memory().unwrap();
        db.with(|c| conversation::ensure(c, Peer::group(30001), "产品组", None)).unwrap();
        db
    }

    #[test]
    fn 静音与取消静音() {
        let db = db();
        let p = Peer::group(30001);
        assert!(!db.with(|c| is_muted(c, p)).unwrap());
        db.tx(|c| set(c, p, true, Some("吵"))).unwrap();
        assert!(db.with(|c| is_muted(c, p)).unwrap());
        db.tx(|c| set(c, p, false, None)).unwrap();
        assert!(!db.with(|c| is_muted(c, p)).unwrap());
        assert_eq!(db.with(|c| count(c)).unwrap(), 0);
    }

    #[test]
    fn 重复静音是幂等的() {
        let db = db();
        let p = Peer::group(30001);
        db.tx(|c| set(c, p, true, None)).unwrap();
        db.tx(|c| set(c, p, true, None)).unwrap();
        assert_eq!(db.with(|c| count(c)).unwrap(), 1);
    }

    #[test]
    fn 私聊与群聊的静音互相独立() {
        let db = db();
        db.tx(|c| set(c, Peer::private(30001), true, None)).unwrap();
        assert!(db.with(|c| is_muted(c, Peer::private(30001))).unwrap());
        assert!(
            !db.with(|c| is_muted(c, Peer::group(30001))).unwrap(),
            "同为 30001 但会话类型不同，绝不能串"
        );
    }

    #[test]
    fn 列表里能看到静音的会话() {
        let db = db();
        db.tx(|c| set(c, Peer::group(30001), true, None)).unwrap();
        assert_eq!(db.with(|c| list(c)).unwrap(), vec![Peer::group(30001)]);
    }
}
