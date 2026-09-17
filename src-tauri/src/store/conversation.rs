//! 会话表（§4.4）。
//!
//! 标签栏的两条规则在这里落地（FR-11 / FR-13）：
//!  · 顺序**固定不自动重排** —— 新会话只会被追加到末尾；
//!  · **手动加入优先级最高**，自动挤占时只挑队尾的非手动标签下手。

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{ConversationDto, Peer, PEER_GROUP, PEER_PRIVATE};
use crate::store::now_ms;

/// 插一个会话（存在则只更新展示名），并把它交给标签栏分配逻辑。
pub fn ensure(conn: &Connection, peer: Peer, name: &str, raw_name: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT INTO conversation(peer_type, peer_id, name, raw_name, unread_count, has_mention,
                                  is_manual_tab, updated_at)
         VALUES(?1, ?2, ?3, ?4, 0, 0, 0, ?5)
         ON CONFLICT(peer_type, peer_id) DO UPDATE SET
             name       = CASE WHEN excluded.name = '' THEN conversation.name ELSE excluded.name END,
             raw_name   = COALESCE(excluded.raw_name, conversation.raw_name),
             updated_at = excluded.updated_at",
        params![peer.peer_type, peer.peer_id, name, raw_name, now_ms()],
    )?;
    Ok(())
}

/// 展示名优先级：备注 > 群名片 > 昵称/群名（FR-14）
pub fn best_name(remark: Option<&str>, card: Option<&str>, nickname: Option<&str>) -> String {
    for c in [remark, card, nickname] {
        if let Some(v) = c {
            if !v.trim().is_empty() {
                return v.to_string();
            }
        }
    }
    String::new()
}

/// 收到一条消息后更新会话的预览字段。
///
/// `inc_unread` 为 false 时不计数（正在查看的会话即时已读，FR-31）。
pub fn touch_last(
    conn: &Connection,
    peer: Peer,
    ts: i64,
    preview: &str,
    sender: Option<&str>,
    inc_unread: bool,
    at_me: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE conversation SET
             last_msg_time   = ?3,
             last_msg_text   = ?4,
             last_msg_sender = ?5,
             unread_count    = unread_count + ?6,
             has_mention     = MAX(has_mention, ?7),
             updated_at      = ?8
         WHERE peer_type = ?1 AND peer_id = ?2",
        params![
            peer.peer_type,
            peer.peer_id,
            ts,
            preview,
            sender,
            if inc_unread { 1 } else { 0 },
            if at_me { 1 } else { 0 },
            now_ms()
        ],
    )?;
    Ok(())
}

pub fn set_name(conn: &Connection, peer: Peer, name: &str) -> Result<()> {
    conn.execute(
        "UPDATE conversation SET name = ?3, updated_at = ?4 WHERE peer_type = ?1 AND peer_id = ?2",
        params![peer.peer_type, peer.peer_id, name, now_ms()],
    )?;
    Ok(())
}

pub fn mark_read(conn: &Connection, peer: Peer) -> Result<()> {
    conn.execute(
        "UPDATE conversation SET unread_count = 0, has_mention = 0, updated_at = ?3
         WHERE peer_type = ?1 AND peer_id = ?2",
        params![peer.peer_type, peer.peer_id, now_ms()],
    )?;
    Ok(())
}

/// 启动时把全部未读清零（FR-39：未读痕迹仅本次运行期间有效，
/// 因为用户很可能已经在手机上读过了）。
pub fn reset_all_unread(conn: &Connection) -> Result<usize> {
    let n = conn.execute("UPDATE conversation SET unread_count = 0, has_mention = 0", [])?;
    Ok(n)
}

pub fn is_muted(conn: &Connection, peer: Peer) -> Result<bool> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM mute WHERE peer_type = ?1 AND peer_id = ?2",
            params![peer.peer_type, peer.peer_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.is_some())
}

fn row_to_dto(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationDto> {
    let muted: i64 = row.get("muted")?;
    Ok(ConversationDto {
        peer_type: row.get("peer_type")?,
        peer_id: row.get("peer_id")?,
        name: row.get("name")?,
        last_msg_time: row.get("last_msg_time")?,
        last_msg_text: row.get("last_msg_text")?,
        last_msg_sender: row.get("last_msg_sender")?,
        unread_count: row.get("unread_count")?,
        has_mention: row.get::<_, i64>("has_mention")? != 0,
        tab_order: row.get("tab_order")?,
        is_manual_tab: row.get::<_, i64>("is_manual_tab")? != 0,
        is_muted: muted != 0,
    })
}

const SELECT: &str = "SELECT c.peer_type, c.peer_id, c.name, c.last_msg_time, c.last_msg_text,
                             c.last_msg_sender, c.unread_count, c.has_mention, c.tab_order,
                             c.is_manual_tab,
                             CASE WHEN m.peer_id IS NULL THEN 0 ELSE 1 END AS muted
                      FROM conversation c
                      LEFT JOIN mute m ON m.peer_type = c.peer_type AND m.peer_id = c.peer_id";

/// 全部会话，按最近消息时间倒序（`···` 列表与折叠条都用它）。
pub fn list(conn: &Connection) -> Result<Vec<ConversationDto>> {
    let sql = format!("{SELECT} ORDER BY c.last_msg_time DESC NULLS LAST, c.peer_id DESC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_dto)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn get(conn: &Connection, peer: Peer) -> Result<Option<ConversationDto>> {
    let sql = format!("{SELECT} WHERE c.peer_type = ?1 AND c.peer_id = ?2");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![peer.peer_type, peer.peer_id], row_to_dto)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// 标签栏里的会话，按 tab_order 升序。**顺序固定不自动重排**（FR-11）。
pub fn list_tabs(conn: &Connection) -> Result<Vec<ConversationDto>> {
    let sql = format!("{SELECT} WHERE c.tab_order IS NOT NULL ORDER BY c.tab_order ASC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_dto)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn tab_count(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM conversation WHERE tab_order IS NOT NULL",
        [],
        |r| r.get(0),
    )?)
}

fn max_tab_order(conn: &Connection) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT COALESCE(MAX(tab_order), -1) FROM conversation WHERE tab_order IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1))
}

/// 把会话放进标签栏的**队尾**。
///
/// `manual` = true 时标记为手动加入：它永不被自动挤占（FR-13）。
/// 超出上限时挤掉队尾的**非手动**标签；如果全是手动的，就让手动加入的这条自己退出。
pub fn push_tab(conn: &Connection, peer: Peer, manual: bool, limit: i64) -> Result<()> {
    let count = tab_count(conn)?;
    if count >= limit {
        // 找队尾的非手动标签
        let victim: Option<(i32, i64)> = conn
            .query_row(
                "SELECT peer_type, peer_id FROM conversation
                 WHERE tab_order IS NOT NULL AND is_manual_tab = 0
                 ORDER BY tab_order DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        match victim {
            Some((t, id)) => {
                conn.execute(
                    "UPDATE conversation SET tab_order = NULL WHERE peer_type = ?1 AND peer_id = ?2",
                    params![t, id],
                )?;
            }
            None => {
                // 全是手动标签：满了就放不进去，直接返回
                return Ok(());
            }
        }
    }

    let order = max_tab_order(conn)? + 1;
    conn.execute(
        "UPDATE conversation SET tab_order = ?3, is_manual_tab = ?4, updated_at = ?5
         WHERE peer_type = ?1 AND peer_id = ?2",
        params![peer.peer_type, peer.peer_id, order, if manual { 1 } else { 0 }, now_ms()],
    )?;
    Ok(())
}

pub fn remove_tab(conn: &Connection, peer: Peer) -> Result<()> {
    conn.execute(
        "UPDATE conversation SET tab_order = NULL, is_manual_tab = 0, updated_at = ?3
         WHERE peer_type = ?1 AND peer_id = ?2",
        params![peer.peer_type, peer.peer_id, now_ms()],
    )?;
    Ok(())
}

/// 手动拖动排序：按传入的 key 顺序重写 tab_order。
pub fn reorder_tabs(conn: &Connection, order: &[String]) -> Result<()> {
    for (i, key) in order.iter().enumerate() {
        let Some(peer) = Peer::parse_key(key) else { continue };
        conn.execute(
            "UPDATE conversation SET tab_order = ?3, updated_at = ?4
             WHERE peer_type = ?1 AND peer_id = ?2 AND tab_order IS NOT NULL",
            params![peer.peer_type, peer.peer_id, i as i64, now_ms()],
        )?;
    }
    Ok(())
}

/// 会话种子的批量写入（FR-14）。返回新建/更新后的会话列表。
pub fn seed_many(conn: &Connection, items: &[(Peer, String, Option<i64>)]) -> Result<()> {
    for (peer, name, ts) in items {
        ensure(conn, *peer, name, None)?;
        if let Some(t) = ts {
            conn.execute(
                "UPDATE conversation SET last_msg_time = MAX(COALESCE(last_msg_time, 0), ?3)
                 WHERE peer_type = ?1 AND peer_id = ?2",
                params![peer.peer_type, peer.peer_id, t],
            )?;
        }
    }
    Ok(())
}

/// 群临时会话：会话名要标注「群临时」（§4.5）
pub fn temp_group_name(group_name: &str, nickname: &str) -> String {
    if group_name.trim().is_empty() {
        format!("{nickname}（群临时）")
    } else {
        format!("{group_name} · {nickname}（群临时）")
    }
}

pub fn peer_type_of(is_group: bool) -> i32 {
    if is_group {
        PEER_GROUP
    } else {
        PEER_PRIVATE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;

    fn db() -> Db {
        Db::open_memory().unwrap()
    }

    fn seed(db: &Db, id: i64) {
        db.with(|c| ensure(c, Peer::group(id), &format!("群{id}"), None)).unwrap();
    }

    #[test]
    fn 展示名优先级_备注高于群名片高于昵称() {
        assert_eq!(best_name(Some("备注"), Some("名片"), Some("昵称")), "备注");
        assert_eq!(best_name(None, Some("名片"), Some("昵称")), "名片");
        assert_eq!(best_name(None, None, Some("昵称")), "昵称");
        assert_eq!(best_name(Some("  "), None, Some("昵称")), "昵称");
    }

    #[test]
    fn ensure_不覆盖已有展示名() {
        let db = db();
        db.with(|c| {
            ensure(c, Peer::group(1), "大前端交流群", None)?;
            ensure(c, Peer::group(1), "", None)?;
            let got = get(c, Peer::group(1))?.unwrap();
            assert_eq!(got.name, "大前端交流群");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 未读与艾特计数只在需要时累加() {
        let db = db();
        db.with(|c| {
            ensure(c, Peer::group(1), "群1", None)?;
            touch_last(c, Peer::group(1), 100, "内容", Some("李工"), true, false)?;
            touch_last(c, Peer::group(1), 200, "@你", Some("李工"), true, true)?;
            let got = get(c, Peer::group(1))?.unwrap();
            assert_eq!(got.unread_count, 2);
            assert!(got.has_mention);
            assert_eq!(got.last_msg_time, Some(200));
            assert_eq!(got.last_msg_text.as_deref(), Some("@你"));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 当前会话不计未读() {
        let db = db();
        db.with(|c| {
            ensure(c, Peer::private(2), "小陈", None)?;
            touch_last(c, Peer::private(2), 100, "在吗", Some("小陈"), false, false)?;
            assert_eq!(get(c, Peer::private(2))?.unwrap().unread_count, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 标记已读清掉未读与艾特() {
        let db = db();
        db.with(|c| {
            ensure(c, Peer::group(1), "群1", None)?;
            touch_last(c, Peer::group(1), 100, "x", None, true, true)?;
            mark_read(c, Peer::group(1))?;
            let got = get(c, Peer::group(1))?.unwrap();
            assert_eq!(got.unread_count, 0);
            assert!(!got.has_mention);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 重启清空全部未读() {
        let db = db();
        db.with(|c| {
            ensure(c, Peer::group(1), "群1", None)?;
            ensure(c, Peer::group(2), "群2", None)?;
            touch_last(c, Peer::group(1), 1, "x", None, true, false)?;
            touch_last(c, Peer::group(2), 2, "y", None, true, false)?;
            assert_eq!(reset_all_unread(c)?, 2);
            assert_eq!(get(c, Peer::group(1))?.unwrap().unread_count, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 标签顺序固定_新会话追加到末尾() {
        let db = db();
        db.with(|c| {
            for id in [1, 2, 3] {
                ensure(c, Peer::group(id), &format!("群{id}"), None)?;
            }
            push_tab(c, Peer::group(1), false, 5)?;
            push_tab(c, Peer::group(2), false, 5)?;
            push_tab(c, Peer::group(3), false, 5)?;

            let tabs = list_tabs(c)?;
            assert_eq!(tabs.iter().map(|t| t.peer_id).collect::<Vec<_>>(), vec![1, 2, 3]);

            // 给群 1 来一条新消息，顺序**不应该**变
            touch_last(c, Peer::group(1), 9999, "新消息", None, true, false)?;
            let tabs = list_tabs(c)?;
            assert_eq!(tabs.iter().map(|t| t.peer_id).collect::<Vec<_>>(), vec![1, 2, 3]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 超出上限时挤掉队尾的非手动标签() {
        let db = db();
        db.with(|c| {
            for id in [1, 2, 3] {
                ensure(c, Peer::group(id), &format!("群{id}"), None)?;
            }
            push_tab(c, Peer::group(1), true, 3)?; // 手动加入
            push_tab(c, Peer::group(2), false, 3)?;
            push_tab(c, Peer::group(3), false, 3)?;
            assert_eq!(tab_count(c)?, 3);

            // 再来一个：应挤掉队尾的非手动标签（群 3），而不是手动加入的群 1
            ensure(c, Peer::group(4), "群4", None)?;
            push_tab(c, Peer::group(4), false, 3)?;

            let ids: Vec<i64> = list_tabs(c)?.iter().map(|t| t.peer_id).collect();
            assert!(ids.contains(&1), "手动加入的标签被挤掉了：{ids:?}");
            assert!(ids.contains(&4));
            assert!(!ids.contains(&3));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 手动加入的会话永不被自动挤占() {
        let db = db();
        db.with(|c| {
            ensure(c, Peer::group(1), "群1", None)?;
            push_tab(c, Peer::group(1), true, 1)?;
            ensure(c, Peer::group(2), "群2", None)?;
            push_tab(c, Peer::group(2), false, 1)?;
            // 全是手动标签且已满 → 新的自动标签放不进去
            let ids: Vec<i64> = list_tabs(c)?.iter().map(|t| t.peer_id).collect();
            assert_eq!(ids, vec![1]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 移出标签后不再出现在标签栏() {
        let db = db();
        db.with(|c| {
            ensure(c, Peer::group(1), "群1", None)?;
            push_tab(c, Peer::group(1), true, 5)?;
            remove_tab(c, Peer::group(1))?;
            let got = get(c, Peer::group(1))?.unwrap();
            assert!(got.tab_order.is_none());
            assert!(!got.is_manual_tab);
            assert_eq!(list(c)?.len(), 1, "会话本身还在，只是不在标签栏");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 拖动排序按传入顺序重写() {
        let db = db();
        db.with(|c| {
            for id in [1, 2, 3] {
                ensure(c, Peer::group(id), &format!("群{id}"), None)?;
            }
            push_tab(c, Peer::group(1), false, 5)?;
            push_tab(c, Peer::group(2), false, 5)?;
            push_tab(c, Peer::group(3), false, 5)?;
            reorder_tabs(c, &["1:3".into(), "1:1".into(), "1:2".into()])?;
            let ids: Vec<i64> = list_tabs(c)?.iter().map(|t| t.peer_id).collect();
            assert_eq!(ids, vec![3, 1, 2]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 静音状态可查() {
        let db = db();
        db.with(|c| {
            ensure(c, Peer::group(1), "群1", None)?;
            assert!(!is_muted(c, Peer::group(1))?);
            crate::store::mute::set(c, Peer::group(1), true, None)?;
            assert!(is_muted(c, Peer::group(1))?);
            let got = get(c, Peer::group(1))?.unwrap();
            assert!(got.is_muted);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 群临时会话名带标注() {
        assert_eq!(temp_group_name("大前端交流群", "路人"), "大前端交流群 · 路人（群临时）");
        assert_eq!(temp_group_name("", "路人"), "路人（群临时）");
    }

    #[test]
    fn 会话列表按最近消息倒序() {
        let db = db();
        db.with(|c| {
            for id in [1, 2, 3] {
                ensure(c, Peer::group(id), &format!("群{id}"), None)?;
            }
            touch_last(c, Peer::group(1), 100, "a", None, true, false)?;
            touch_last(c, Peer::group(3), 300, "c", None, true, false)?;
            touch_last(c, Peer::group(2), 200, "b", None, true, false)?;
            let ids: Vec<i64> = list(c)?.iter().map(|t| t.peer_id).collect();
            assert_eq!(ids, vec![3, 2, 1]);
            Ok(())
        })
        .unwrap();
    }
}
