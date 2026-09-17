//! 消息表：幂等写入、历史分页、墓碑过滤（§4.4 / §4.8）。
//!
//! 两个必须记住的点：
//!  1. `message_id` 唯一，写入统一走 `INSERT ... ON CONFLICT(message_id) DO UPDATE`。
//!     所以"同一 message_id 被事件与历史拉取各送一次"（以及 `message_sent.*` 回传）
//!     都不会产生重复 —— 幂等是这一层给的，不是上层小心翼翼地避免重复调用换来的；
//!  2. **所有读取路径都要按 tombstone 过滤**。只删 message 行的话，下次向上翻历史
//!     或重启补齐时 NapCat 会把这条消息原样送回来（"删了又出现"）。
//!
//! `images` 里的 sha256 直接从本地路径的文件名推出来 —— 媒体文件就是按 `<sha256>.<ext>`
//! 命名的（§4.4），所以不必再往消息段里塞一份哈希。

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

use crate::model::{
    image_state, seg_types, MessageDto, Peer, ReplyPreview, Seg,
};
use crate::store::now_ms;

/// 写入用的输入（由 `ob::event` 构造）
#[derive(Clone, Debug)]
pub struct NewMessage {
    pub message_id: String,
    pub peer: Peer,
    pub ts: i64,
    pub sender_id: i64,
    pub sender_name: Option<String>,
    pub is_self: bool,
    pub segments: Vec<Seg>,
    pub reply_to: Option<String>,
    pub is_at_me: bool,
    pub send_state: i32,
}

impl NewMessage {
    pub fn text_preview(&self) -> String {
        let joined: String = self.segments.iter().map(|s| s.as_plain()).collect();
        flatten(&joined)
    }
}

/// 单行化：折叠条与列表项都是单行展示，换行会破坏布局（§3.2）
pub fn flatten(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' || ch == '\u{2028}' || ch == '\u{2029}' {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
            continue;
        }
        if ch == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(ch);
    }
    out.trim().to_string()
}

/// 从媒体路径反推 sha256：媒体文件名就是 `<sha256>.<ext>`
pub fn sha_from_path(path: &str) -> Option<String> {
    let file = path.rsplit(['/', '\\']).next()?;
    let stem = file.split('.').next()?;
    if stem.len() == 64 && stem.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(stem.to_ascii_lowercase())
    } else {
        None
    }
}

/// 该会话下一个自增序号。稳定排序的兜底（ts 相同的两条消息靠它定先后）。
pub fn next_seq(conn: &Connection, peer: Peer) -> Result<i64> {
    let n: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM message WHERE peer_type = ?1 AND peer_id = ?2",
        params![peer.peer_type, peer.peer_id],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// 幂等写入。返回 `true` 表示这是新消息（不是重复投递）。
pub fn upsert(conn: &Connection, m: &NewMessage) -> Result<bool> {
    let segs_json = serde_json::to_string(&m.segments)?;
    let types = seg_types(&m.segments);
    let text = m.text_preview();
    let has_image = m.segments.iter().any(|s| matches!(s, Seg::Image { .. }));
    let image_state_v = if has_image { image_state::NONE } else { image_state::NONE };
    let seq = next_seq(conn, m.peer)?;

    let existed: bool = conn
        .query_row(
            "SELECT 1 FROM message WHERE message_id = ?1",
            params![m.message_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .is_some();

    conn.execute(
        "INSERT INTO message(message_id, peer_type, peer_id, seq, ts, sender_id, sender_name,
                             is_self, text, segments, seg_types, reply_to, is_at_me, has_image,
                             image_state, send_state, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
         ON CONFLICT(message_id) DO UPDATE SET
             sender_name = COALESCE(excluded.sender_name, message.sender_name),
             text        = excluded.text,
             segments    = excluded.segments,
             seg_types   = excluded.seg_types,
             reply_to    = COALESCE(excluded.reply_to, message.reply_to),
             is_at_me    = MAX(message.is_at_me, excluded.is_at_me),
             has_image   = excluded.has_image,
             send_state  = excluded.send_state",
        params![
            m.message_id,
            m.peer.peer_type,
            m.peer.peer_id,
            seq,
            m.ts,
            m.sender_id,
            m.sender_name,
            crate::store::b2i(m.is_self),
            text,
            segs_json,
            types,
            m.reply_to,
            crate::store::b2i(m.is_at_me),
            crate::store::b2i(has_image),
            image_state_v,
            m.send_state,
            now_ms()
        ],
    )?;
    Ok(!existed)
}

/// 撤回：标记但**保留原文**（FR-44）
pub fn mark_recalled(conn: &Connection, message_id: &str, operator_id: Option<i64>) -> Result<bool> {
    let n = conn.execute(
        "UPDATE message SET is_recalled = 1, recalled_by = ?2, recalled_at = ?3
         WHERE message_id = ?1",
        params![message_id, operator_id, now_ms()],
    )?;
    Ok(n > 0)
}

pub fn set_send_state(conn: &Connection, message_id: &str, state: i32) -> Result<()> {
    conn.execute(
        "UPDATE message SET send_state = ?2 WHERE message_id = ?1",
        params![message_id, state],
    )?;
    Ok(())
}

pub fn set_image_state(conn: &Connection, message_id: &str, state: i32) -> Result<()> {
    conn.execute(
        "UPDATE message SET image_state = ?2 WHERE message_id = ?1",
        params![message_id, state],
    )?;
    Ok(())
}

/// 图片下载完成后把路径补回消息段（§4.8 关键算法 #6）。
///
/// 图片是"收到即入队下载"，所以消息先入库、路径后到；这里按段下标精确回填，
/// 顺手把整条消息的 image_state 也更新掉。
pub fn patch_image_path(
    conn: &Connection,
    message_id: &str,
    seg_index: usize,
    abs_path: Option<&str>,
    state: i32,
) -> Result<()> {
    let seg_json: Option<String> = conn
        .query_row(
            "SELECT segments FROM message WHERE message_id = ?1",
            params![message_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(seg_json) = seg_json else { return Ok(()) };

    let Ok(mut segs) = serde_json::from_str::<Vec<Seg>>(&seg_json) else {
        return Ok(());
    };
    match segs.get_mut(seg_index) {
        Some(Seg::Image { path, state: st, .. }) => {
            *path = abs_path.map(|s| s.to_string());
            *st = state;
        }
        _ => return Ok(()),
    }

    // 整条消息的状态：全部成功才算已落盘，否则按最差的那个算
    let overall = if segs.iter().any(|s| matches!(s, Seg::Image { state: image_state::FAILED, .. }))
    {
        image_state::FAILED
    } else if segs.iter().any(|s| matches!(s, Seg::Image { state: image_state::DOWNLOADING, .. })) {
        image_state::DOWNLOADING
    } else if segs.iter().any(|s| matches!(s, Seg::Image { state: image_state::READY, .. })) {
        image_state::READY
    } else {
        image_state::NONE
    };

    conn.execute(
        "UPDATE message SET segments = ?2, image_state = ?3 WHERE message_id = ?1",
        params![message_id, serde_json::to_string(&segs)?, overall],
    )?;
    Ok(())
}

/// 物理删除一行（手工删除或清空会话时用）。**调用方必须先写墓碑**。
pub fn delete(conn: &Connection, message_id: &str) -> Result<()> {
    conn.execute("DELETE FROM message WHERE message_id = ?1", params![message_id])?;
    Ok(())
}

pub fn count(conn: &Connection, peer: Peer) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM message WHERE peer_type = ?1 AND peer_id = ?2",
        params![peer.peer_type, peer.peer_id],
        |r| r.get(0),
    )?)
}

/// 单条读取（引用解析、`get_msg` 兜底用）
pub fn get(conn: &Connection, message_id: &str) -> Result<Option<MessageDto>> {
    let sql = format!("{SELECT} WHERE m.message_id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![message_id], raw_row)?;
    let mut out = hydrate(conn, rows.collect::<rusqlite::Result<Vec<_>>>()?)?;
    Ok(out.pop())
}

/// 历史分页：按 seq 向上取一页（FR-20）。已墓碑的消息在这里就被过滤掉。
pub fn list_page(
    conn: &Connection,
    peer: Peer,
    before_seq: Option<i64>,
    limit: i64,
) -> Result<Vec<MessageDto>> {
    let sql = format!(
        "{SELECT} WHERE m.peer_type = ?1 AND m.peer_id = ?2
                  AND m.message_id NOT IN (SELECT message_id FROM tombstone)
                  AND (?3 IS NULL OR m.seq < ?3)
         ORDER BY m.seq DESC LIMIT ?4"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![peer.peer_type, peer.peer_id, before_seq, limit], raw_row)?;
    let mut list = hydrate(conn, rows.collect::<rusqlite::Result<Vec<_>>>()?)?;
    list.reverse(); // 查询是倒序取，返回给前端要正序
    Ok(list)
}

/// 最近一页（首次展开会话时用）
pub fn list_latest(conn: &Connection, peer: Peer, limit: i64) -> Result<Vec<MessageDto>> {
    list_page(conn, peer, None, limit)
}

/// 会话内是否还有更早的消息（决定"以上是全部"要不要显示）
pub fn has_older(conn: &Connection, peer: Peer, seq: i64) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM message
         WHERE peer_type = ?1 AND peer_id = ?2 AND seq < ?3
           AND message_id NOT IN (SELECT message_id FROM tombstone)",
        params![peer.peer_type, peer.peer_id, seq],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

const SELECT: &str = "SELECT m.message_id, m.peer_type, m.peer_id, m.seq, m.ts, m.sender_id,
                             m.sender_name, m.is_self, m.text, m.segments, m.reply_to,
                             m.is_at_me, m.has_image, m.image_state, m.is_recalled,
                             m.recalled_by, m.send_state
                      FROM message m";

#[derive(Debug)]
struct RawRow {
    message_id: String,
    peer_type: i32,
    peer_id: i64,
    seq: Option<i64>,
    ts: i64,
    sender_id: i64,
    sender_name: Option<String>,
    is_self: bool,
    text: Option<String>,
    segments: String,
    reply_to: Option<String>,
    is_at_me: bool,
    has_image: bool,
    image_state_v: i32,
    is_recalled: bool,
    recalled_by: Option<i64>,
    send_state: i32,
}

fn raw_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        message_id: row.get(0)?,
        peer_type: row.get(1)?,
        peer_id: row.get(2)?,
        seq: row.get(3)?,
        ts: row.get(4)?,
        sender_id: row.get(5)?,
        sender_name: row.get(6)?,
        is_self: row.get::<_, i64>(7)? != 0,
        text: row.get(8)?,
        segments: row.get(9)?,
        reply_to: row.get(10)?,
        is_at_me: row.get::<_, i64>(11)? != 0,
        has_image: row.get::<_, i64>(12)? != 0,
        image_state_v: row.get(13)?,
        is_recalled: row.get::<_, i64>(14)? != 0,
        recalled_by: row.get(15)?,
        send_state: row.get(16)?,
    })
}

/// 把裸行补全成前端要的 DTO：解析消息段、补引用摘要、补图片引用。
/// 批次查询，避免 N+1。
fn hydrate(conn: &Connection, rows: Vec<RawRow>) -> Result<Vec<MessageDto>> {
    // 1) 引用目标
    let reply_ids: Vec<String> = rows.iter().filter_map(|r| r.reply_to.clone()).collect();
    let replies = load_reply_previews(conn, &reply_ids)?;

    // 2) 图片
    let mut sha_list: Vec<String> = Vec::new();
    let mut parsed: Vec<Vec<Seg>> = Vec::with_capacity(rows.len());
    for r in &rows {
        let segs: Vec<Seg> = serde_json::from_str(&r.segments).unwrap_or_else(|_| vec![]);
        for s in &segs {
            if let Seg::Image { path: Some(p), .. } = s {
                if let Some(sha) = sha_from_path(p) {
                    sha_list.push(sha);
                }
            }
        }
        parsed.push(segs);
    }
    let images = crate::store::image::load_refs(conn, &sha_list)?;

    // 3) 撤回者的名字
    let recallers: Vec<i64> = rows.iter().filter_map(|r| r.recalled_by).collect();
    let recaller_names = load_sender_names(conn, &recallers)?;

    let mut out = Vec::with_capacity(rows.len());
    for (i, r) in rows.into_iter().enumerate() {
        let segs = parsed.get(i).cloned().unwrap_or_default();
        let img_refs = segs
            .iter()
            .filter_map(|s| match s {
                Seg::Image { path: Some(p), .. } => {
                    sha_from_path(p).and_then(|sha| images.get(&sha).cloned())
                }
                _ => None,
            })
            .collect();

        let reply_to = r.reply_to.clone();
        let reply_preview = reply_to
            .as_ref()
            .map(|id| replies.get(id).cloned().unwrap_or(ReplyPreview {
                message_id: id.clone(),
                sender_name: None,
                summary: String::new(),
                deleted: false,
            }));

        out.push(MessageDto {
            message_id: r.message_id,
            peer_type: r.peer_type,
            peer_id: r.peer_id,
            seq: r.seq,
            ts: r.ts,
            sender_id: r.sender_id,
            sender_name: r.sender_name,
            is_self: r.is_self,
            text: r.text,
            segments: segs,
            reply_to,
            reply_preview,
            is_at_me: r.is_at_me,
            has_image: r.has_image,
            image_state: r.image_state_v,
            images: img_refs,
            is_recalled: r.is_recalled,
            recalled_by: r.recalled_by,
            recalled_by_name: r
                .recalled_by
                .and_then(|id| recaller_names.get(&id).cloned()),
            send_state: r.send_state,
        });
    }
    Ok(out)
}

/// 引用摘要：墓碑命中的话直接把 deleted 标上（FR-45：引用块降级为「引用的消息已被删除」）
fn load_reply_previews(
    conn: &Connection,
    ids: &[String],
) -> Result<HashMap<String, ReplyPreview>> {
    let mut out = HashMap::new();
    for id in ids {
        if out.contains_key(id) {
            continue;
        }
        let deleted = crate::store::tombstone::is_deleted(conn, id)?;
        if deleted {
            out.insert(
                id.clone(),
                ReplyPreview {
                    message_id: id.clone(),
                    sender_name: None,
                    summary: String::new(),
                    deleted: true,
                },
            );
            continue;
        }
        let got: Option<(Option<String>, Option<String>, Vec<Seg>)> = conn
            .query_row(
                "SELECT sender_name, text, segments FROM message WHERE message_id = ?1",
                params![id],
                |r| {
                    let seg_json: String = r.get(2)?;
                    let segs: Vec<Seg> = serde_json::from_str(&seg_json).unwrap_or_default();
                    Ok((r.get(0)?, r.get(1)?, segs))
                },
            )
            .optional()?;

        if let Some((sender_name, text, segs)) = got {
            let summary = text.unwrap_or_else(|| {
                segs.iter().map(|s| s.as_plain()).collect::<String>()
            });
            out.insert(
                id.clone(),
                ReplyPreview {
                    message_id: id.clone(),
                    sender_name,
                    summary: flatten(&summary),
                    deleted: false,
                },
            );
        } else {
            // 本地没有这条消息：对端可能已经 LRU 掉了，只能降级
            out.insert(
                id.clone(),
                ReplyPreview {
                    message_id: id.clone(),
                    sender_name: None,
                    summary: String::new(),
                    deleted: true,
                },
            );
        }
    }
    Ok(out)
}

/// 一批 user_id 对应的展示名（群名片优先），用于"XX 撤回了 YY 的消息"
fn load_sender_names(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, String>> {
    let mut out = HashMap::new();
    for id in ids {
        let name: Option<String> = conn
            .query_row(
                "SELECT COALESCE(sender_name, '') FROM message WHERE sender_id = ?1
                 ORDER BY ts DESC LIMIT 1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(n) = name {
            if !n.is_empty() {
                out.insert(*id, n);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{conversation, tombstone, Db};
    use anyhow::Result;

    fn db() -> Db {
        Db::open_memory().unwrap()
    }

    fn peer() -> Peer {
        Peer::group(30001)
    }

    fn new_msg(id: &str, ts: i64) -> NewMessage {
        NewMessage {
            message_id: id.into(),
            peer: peer(),
            ts,
            sender_id: 30011,
            sender_name: Some("李工".into()),
            is_self: false,
            segments: vec![Seg::text("你好")],
            reply_to: None,
            is_at_me: false,
            send_state: 0,
        }
    }

    fn setup(db: &Db) {
        db.with(|c| conversation::ensure(c, peer(), "大前端交流群", None)).unwrap();
    }

    #[test]
    fn 幂等_同一_message_id_写两次只有一行() {
        let db = db();
        setup(&db);
        db.tx(|c| {
            assert!(upsert(c, &new_msg("1", 100))?, "第一次是新消息");
            assert!(!upsert(c, &new_msg("1", 100))?, "第二次是重复投递");
            assert_eq!(count(c, peer())?, 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn 幂等_历史拉取与事件各送一次不产生重复() -> Result<()> {
        let db = db();
        setup(&db);
        let mut m = new_msg("2", 200);
        db.tx(|c| upsert(c, &m))?;
        // 向上翻历史时远端把同一条又送了一遍，字段略有差异
        m.segments = vec![Seg::text("你好（远端版本）")];
        db.tx(|c| upsert(c, &m))?;
        let page = db.with(|c| list_latest(c, peer(), 10))?;
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].text.as_deref(), Some("你好（远端版本）"));
        Ok(())
    }

    #[test]
    fn 自己的消息回传_按_message_id_覆盖乐观条目() {
        let db = db();
        setup(&db);
        db.tx(|c| {
            // 本地乐观条目
            let mut local = new_msg("local:1", 300);
            local.is_self = true;
            local.sender_id = 10001;
            local.send_state = crate::model::send_state::LOCAL;
            upsert(c, &local)?;
            Ok(())
        })
        .unwrap();

        db.tx(|c| {
            // message_sent.* 回传真实 id
            let mut real = new_msg("88", 300);
            real.is_self = true;
            real.sender_id = 10001;
            real.send_state = crate::model::send_state::CONFIRMED;
            upsert(c, &real)?;
            delete(c, "local:1")?;
            Ok(())
        })
        .unwrap();

        let page = db.with(|c| list_latest(c, peer(), 10)).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].message_id, "88");
        assert_eq!(page[0].send_state, crate::model::send_state::CONFIRMED);
    }

    #[test]
    fn 墓碑_删除的消息不会再被历史拉回来() -> Result<()> {
        let db = db();
        setup(&db);
        db.tx(|c| {
            upsert(c, &new_msg("10", 1000))?;
            upsert(c, &new_msg("11", 2000))?;
            Ok(())
        })
        .unwrap();

        // 手动删除：先写墓碑，再删行
        db.tx(|c| {
            tombstone::add(c, "10", peer(), Some("手滑"))?;
            delete(c, "10")?;
            Ok(())
        })
        .unwrap();

        // 远端补齐又把这条送回来了
        db.tx(|c| upsert(c, &new_msg("10", 1000)))?;

        let page = db.with(|c| list_latest(c, peer(), 10))?;
        let ids: Vec<&str> = page.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(ids, vec!["11"], "墓碑必须把删掉的消息挡在外面");
        Ok(())
    }

    #[test]
    fn 分页_向上翻页取更早的一页且升序返回() -> Result<()> {
        let db = db();
        setup(&db);
        db.tx(|c| {
            for i in 1..=10 {
                upsert(c, &new_msg(&i.to_string(), i * 100))?;
            }
            Ok(())
        })
        .unwrap();

        let first = db.with(|c| list_latest(c, peer(), 4)).unwrap();
        assert_eq!(
            first.iter().map(|m| m.message_id.as_str()).collect::<Vec<_>>(),
            vec!["7", "8", "9", "10"]
        );

        let older = db
            .with(|c| list_page(c, peer(), first[0].seq, 4))
            .unwrap();
        assert_eq!(
            older.iter().map(|m| m.message_id.as_str()).collect::<Vec<_>>(),
            vec!["3", "4", "5", "6"]
        );
        Ok(())
    }

    #[test]
    fn 分页_没有更早的了() -> Result<()> {
        let db = db();
        setup(&db);
        db.tx(|c| upsert(c, &new_msg("1", 100)))?;
        let page = db.with(|c| list_latest(c, peer(), 10))?;
        assert!(!db.with(|c| has_older(c, peer(), page[0].seq.unwrap()))?);
        Ok(())
    }

    #[test]
    fn 撤回_保留原文并记录执行者() -> Result<()> {
        let db = db();
        setup(&db);
        db.tx(|c| upsert(c, &new_msg("20", 100)))?;
        db.tx(|c| mark_recalled(c, "20", Some(30012)))?;
        let m = db.with(|c| get(c, "20"))?.unwrap();
        assert!(m.is_recalled);
        assert_eq!(m.recalled_by, Some(30012));
        assert_eq!(m.text.as_deref(), Some("你好"), "撤回必须保留原文");
        Ok(())
    }

    #[test]
    fn 引用_被删除时降级为已删除() -> Result<()> {
        let db = db();
        setup(&db);
        db.tx(|c| {
            upsert(c, &new_msg("30", 100))?;
            let mut reply = new_msg("31", 200);
            reply.reply_to = Some("30".into());
            upsert(c, &reply)?;
            Ok(())
        })
        .unwrap();

        let m = db.with(|c| get(c, "31")).unwrap().unwrap();
        let rp = m.reply_preview.unwrap();
        assert!(!rp.deleted);
        assert_eq!(rp.sender_name.as_deref(), Some("李工"));
        assert_eq!(rp.summary, "你好");

        // 引用目标被墓碑删除
        db.tx(|c| {
            tombstone::add(c, "30", peer(), None)?;
            delete(c, "30")?;
            Ok(())
        })
        .unwrap();
        let m = db.with(|c| get(c, "31")).unwrap().unwrap();
        assert!(m.reply_preview.unwrap().deleted);
        Ok(())
    }

    #[test]
    fn 摘要_多段合并并单行化() {
        let mut m = new_msg("40", 100);
        m.segments = vec![
            Seg::At { qq: 10001, name: Some("你".into()), is_self: true },
            Seg::text(" 第一行\n第二行"),
            Seg::Image { path: None, sub_type: 0, state: image_state::READY },
        ];
        assert_eq!(m.text_preview(), "@你 第一行 第二行[图片]");
    }

    #[test]
    fn 从媒体路径反推_sha256() {
        let sha = "a".repeat(64);
        let path = format!("C:/Users/x/AppData/Local/qq-drawer/media/aa/{sha}.png");
        assert_eq!(sha_from_path(&path).unwrap(), sha);
        assert!(sha_from_path("C:/tmp/not-a-hash.png").is_none());
        assert!(sha_from_path("C:/tmp/abc.png").is_none());
    }

    #[test]
    fn 艾特标记_只增不减() {
        let db = db();
        setup(&db);
        db.tx(|c| {
            let mut m = new_msg("50", 100);
            m.is_at_me = true;
            upsert(c, &m)?;
            m.is_at_me = false; // 历史拉回来时丢了 @ 信息
            upsert(c, &m)?;
            Ok(())
        })
        .unwrap();
        assert!(db.with(|c| get(c, "50")).unwrap().unwrap().is_at_me);
    }

    #[test]
    fn 图片路径回填后整条消息状态变已落盘() -> Result<()> {
        let db = db();
        setup(&db);
        db.tx(|c| {
            let mut m = new_msg("70", 100);
            m.segments = vec![
                Seg::text("看图"),
                Seg::Image { path: None, sub_type: 0, state: image_state::DOWNLOADING },
            ];
            upsert(c, &m)
                .map(|_| ())
        })?;
        db.tx(|c| patch_image_path(c, "70", 1, Some("C:/m/aa/xxx.png"), image_state::READY))?;

        let m = db.with(|c| get(c, "70"))?.unwrap();
        assert_eq!(m.image_state, image_state::READY);
        match &m.segments[1] {
            Seg::Image { path, state, .. } => {
                assert_eq!(path.as_deref(), Some("C:/m/aa/xxx.png"));
                assert_eq!(*state, image_state::READY);
            }
            other => panic!("段类型错了: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn 会话序号自增且隔离() -> Result<()> {
        let db = db();
        setup(&db);
        db.with(|c| conversation::ensure(c, Peer::private(20001), "小陈", None)).unwrap();
        db.tx(|c| {
            upsert(c, &new_msg("60", 100))?;
            upsert(c, &new_msg("61", 200))?;
            Ok(())
        })
        .unwrap();
        let seq = db.with(|c| next_seq(c, peer())).unwrap();
        assert_eq!(seq, 3);
        let other = db.with(|c| next_seq(c, Peer::private(20001))).unwrap();
        assert_eq!(other, 1, "序号是会话内自增，不跨会话");
        Ok(())
    }
}
