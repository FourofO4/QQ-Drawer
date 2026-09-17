//! 群成员缓存（@ 选人与名字解析）。
//!
//! 官方建议"缓存 + 增量更新"，照做：本地缓存 24h 过期刷新（§4.6 / FR-27）。
//! **不存头像** —— 成员列表不显示头像，几百个头像的网络与解码开销纯浪费（优化清单 #6）。

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::model::MemberDto;
use crate::store::now_ms;

pub const TTL_MS: i64 = 24 * 60 * 60 * 1000;

pub fn upsert_many(
    conn: &Connection,
    group_id: i64,
    items: &[(i64, Option<String>, Option<String>, Option<String>)],
) -> Result<()> {
    let now = now_ms();
    for (user_id, nickname, card, role) in items {
        conn.execute(
            "INSERT INTO member_cache(group_id, user_id, nickname, card, role, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(group_id, user_id) DO UPDATE SET
                 nickname   = excluded.nickname,
                 card       = excluded.card,
                 role       = excluded.role,
                 updated_at = excluded.updated_at",
            params![group_id, user_id, nickname, card, role, now],
        )?;
    }
    Ok(())
}

/// 展示名：群名片 > 昵称 > QQ 号
pub fn display_name(card: Option<&str>, nickname: Option<&str>, user_id: i64) -> String {
    for c in [card, nickname] {
        if let Some(v) = c {
            if !v.trim().is_empty() {
                return v.to_string();
            }
        }
    }
    user_id.to_string()
}

/// 列表过滤（@ 选人的搜索，FR-27）
pub fn list(conn: &Connection, group_id: i64, query: &str, limit: i64) -> Result<Vec<MemberDto>> {
    let like = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT user_id, nickname, card FROM member_cache
         WHERE group_id = ?1
           AND (?2 = '' OR card LIKE ?3 OR nickname LIKE ?3 OR CAST(user_id AS TEXT) LIKE ?3)
         ORDER BY (card IS NULL), card, nickname
         LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![group_id, query.trim(), like, limit], |r| {
        let user_id: i64 = r.get(0)?;
        let nickname: Option<String> = r.get(1)?;
        let card: Option<String> = r.get(2)?;
        Ok(MemberDto {
            user_id,
            nickname: nickname.clone(),
            card: card.clone(),
            display_name: display_name(card.as_deref(), nickname.as_deref(), user_id),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 单个人名（@ 令牌的显示名兜底）
pub fn name_of(conn: &Connection, group_id: i64, user_id: i64) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT nickname, card FROM member_cache WHERE group_id = ?1 AND user_id = ?2",
    )?;
    let mut rows = stmt.query(params![group_id, user_id])?;
    Ok(match rows.next()? {
        Some(r) => {
            let nickname: Option<String> = r.get(0)?;
            let card: Option<String> = r.get(1)?;
            Some(display_name(card.as_deref(), nickname.as_deref(), user_id))
        }
        None => None,
    })
}

/// 缓存是否需要刷新（24h 过期）
pub fn is_stale(conn: &Connection, group_id: i64) -> Result<bool> {
    let newest: Option<i64> = conn
        .query_row(
            "SELECT MAX(updated_at) FROM member_cache WHERE group_id = ?1",
            params![group_id],
            |r| r.get(0),
        )
        .unwrap_or(None);
    Ok(match newest {
        None => true,
        Some(t) => now_ms() - t > TTL_MS,
    })
}

/// 群成员增减时增量更新（§4.5 notice.group_increase / decrease）
pub fn remove(conn: &Connection, group_id: i64, user_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM member_cache WHERE group_id = ?1 AND user_id = ?2",
        params![group_id, user_id],
    )?;
    Ok(())
}

pub fn count(conn: &Connection, group_id: i64) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM member_cache WHERE group_id = ?1",
        params![group_id],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;
    use anyhow::Result;

    fn db() -> Db {
        let db = Db::open_memory().unwrap();
        db.tx(|c| {
            upsert_many(
                c,
                30001,
                &[
                    (30011, Some("李工".into()), Some("李工".into()), None),
                    (30012, Some("小陈".into()), None, None),
                    (30013, Some("王姐".into()), Some("王姐（实习）".into()), None),
                ],
            )
        })
        .unwrap();
        db
    }

    #[test]
    fn 展示名优先级_群名片高于昵称() {
        assert_eq!(display_name(Some("王姐（实习）"), Some("王姐"), 30013), "王姐（实习）");
        assert_eq!(display_name(None, Some("小陈"), 30012), "小陈");
        assert_eq!(display_name(Some("  "), None, 999), "999");
    }

    #[test]
    fn 空关键词返回全部() {
        let db = db();
        assert_eq!(db.with(|c| list(c, 30001, "", 10)).unwrap().len(), 3);
    }

    #[test]
    fn 按名字过滤() {
        let db = db();
        let got = db.with(|c| list(c, 30001, "工", 10)).unwrap();
        assert_eq!(got.iter().map(|m| m.user_id).collect::<Vec<_>>(), vec![30011]);
    }

    #[test]
    fn 按_QQ_号过滤() {
        let db = db();
        let got = db.with(|c| list(c, 30001, "30012", 10)).unwrap();
        assert_eq!(got[0].display_name, "小陈");
    }

    #[test]
    fn 限制返回条数() {
        let db = db();
        assert_eq!(db.with(|c| list(c, 30001, "", 2)).unwrap().len(), 2);
    }

    #[test]
    fn 单个人名可查() {
        let db = db();
        assert_eq!(
            db.with(|c| name_of(c, 30001, 30013)).unwrap().as_deref(),
            Some("王姐（实习）")
        );
        assert!(db.with(|c| name_of(c, 30001, 12345)).unwrap().is_none());
    }

    #[test]
    fn 缓存过期判定() {
        let db = db();
        assert!(!db.with(|c| is_stale(c, 30001)).unwrap());
        assert!(db.with(|c| is_stale(c, 40000)).unwrap(), "没有缓存视为过期");
    }

    #[test]
    fn 成员退群后缓存被移除() -> Result<()> {
        let db = db();
        db.tx(|c| remove(c, 30001, 30012))?;
        assert_eq!(db.with(|c| count(c, 30001)).unwrap(), 2);
        assert!(db.with(|c| name_of(c, 30001, 30012)).unwrap().is_none());
        Ok(())
    }
}
