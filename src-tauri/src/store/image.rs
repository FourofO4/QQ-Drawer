//! 图片索引与 LRU（§4.4 / FR-41 / FR-42）。
//!
//! `image_cache` 的键是 **sha256 而不是消息 id** —— 同一张图被多条消息引用时只存一份，
//! 这也是"文件名用哈希"的原因。
//!
//! 本库是图片的唯一副本：NapCat 侧的文件标识同样受 LRU 管理，清理 = 永久丢失（§4.4）。

use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashMap;

use crate::model::{CacheOverviewDto, CacheStatDto, ImageRef, Peer};
use crate::store::now_ms;

/// 落盘后登记（同图重复到达时只刷新访问时间）
pub fn register(
    conn: &Connection,
    sha256: &str,
    rel_path: &str,
    ext: &str,
    bytes: i64,
    width: Option<i64>,
    height: Option<i64>,
    sub_type: i32,
) -> Result<()> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO image_cache(sha256, rel_path, ext, bytes, width, height, sub_type,
                                 ref_count, created_at, last_access_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8)
         ON CONFLICT(sha256) DO UPDATE SET
             rel_path       = excluded.rel_path,
             ref_count      = image_cache.ref_count + 1,
             last_access_at = excluded.last_access_at",
        params![sha256, rel_path, ext, bytes, width, height, sub_type, now],
    )?;
    Ok(())
}

/// 渲染时刷新 LRU 时间戳（优化清单：只对图片做 LRU）
pub fn touch(conn: &Connection, sha256: &str) -> Result<()> {
    conn.execute(
        "UPDATE image_cache SET last_access_at = ?2 WHERE sha256 = ?1",
        params![sha256, now_ms()],
    )?;
    Ok(())
}

/// 批量取图片引用信息（消息 DTO 的 `images` 字段）
pub fn load_refs(conn: &Connection, shas: &[String]) -> Result<HashMap<String, ImageRef>> {
    let mut out = HashMap::new();
    for sha in shas {
        if out.contains_key(sha) {
            continue;
        }
        let got = conn
            .query_row(
                "SELECT rel_path, width, height, sub_type FROM image_cache WHERE sha256 = ?1",
                params![sha],
                |r| {
                    Ok(ImageRef {
                        sha256: sha.clone(),
                        rel_path: r.get(0)?,
                        width: r.get(1)?,
                        height: r.get(2)?,
                        sub_type: r.get(3)?,
                    })
                },
            )
            .ok();
        if let Some(r) = got {
            out.insert(sha.clone(), r);
        }
    }
    Ok(out)
}

pub fn rel_path(conn: &Connection, sha256: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT rel_path FROM image_cache WHERE sha256 = ?1")?;
    let mut rows = stmt.query(params![sha256])?;
    Ok(match rows.next()? {
        Some(r) => Some(r.get(0)?),
        None => None,
    })
}

/// 这张图**还被多少条消息引用**（真正去重后的口径，不是 `register` 的调用次数）。
///
/// 为什么不能直接用 `image_cache.ref_count`：那是"落盘事件的计数"，
/// 同一条消息被重复投递、或者重连补齐时命中同一张图，都会把它抬高。
/// 而删除单条消息时判断"能不能顺手把文件删掉"必须按**消息**算——
/// 误删会让别的会话里还在用的图变成 `[图片已清除]`（§4.8 #10）。
///
/// 实现：图片分片的本地路径一定含 `<sha256>`（文件名就是哈希），
/// 所以在 `segments` 上做子串匹配是精确的。
pub fn referenced_by(conn: &Connection, sha256: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM message WHERE has_image = 1 AND segments LIKE '%' || ?1 || '%'",
        params![sha256],
        |r| r.get(0),
    )?)
}

pub fn total(conn: &Connection) -> Result<(i64, i64)> {
    let row = conn.query_row(
        "SELECT COALESCE(SUM(bytes), 0), COUNT(*) FROM image_cache",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(row)
}

/// 按会话分组的占用。
///
/// 实现说明：图片是按 sha 去重的，同一张图可能被多个会话引用，所以**分组数据会有重叠**，
/// 真正的"总量"取自 `image_cache` 本身（`total`），不是分组求和。
/// 这里为了不动表结构，用扫描消息段的方式反查会话与图片的关系——只在打开缓存管理页时跑。
pub fn stats_by_conversation(conn: &Connection) -> Result<Vec<CacheStatDto>> {
    let mut stmt = conn.prepare(
        "SELECT m.peer_type, m.peer_id, COALESCE(c.name, ''), m.segments
         FROM message m
         LEFT JOIN conversation c ON c.peer_type = m.peer_type AND c.peer_id = m.peer_id
         WHERE m.has_image = 1",
    )?;

    let mut per_peer: HashMap<(i32, i64), (String, Vec<String>)> = HashMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i32>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;

    for row in rows {
        let (pt, pid, name, seg_json) = row?;
        let segs: Vec<crate::model::Seg> = serde_json::from_str(&seg_json).unwrap_or_default();
        let entry = per_peer.entry((pt, pid)).or_insert_with(|| (name, Vec::new()));
        for s in segs {
            if let crate::model::Seg::Image { path: Some(p), .. } = s {
                if let Some(sha) = crate::store::message::sha_from_path(&p) {
                    entry.1.push(sha);
                }
            }
        }
    }

    let mut out = Vec::new();
    for ((pt, pid), (name, shas)) in per_peer {
        let mut bytes = 0i64;
        let mut count = 0i64;
        let mut oldest: Option<i64> = None;
        let mut newest: Option<i64> = None;
        for sha in &shas {
            if let Ok((b, created)) = conn.query_row(
                "SELECT bytes, created_at FROM image_cache WHERE sha256 = ?1",
                params![sha],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            ) {
                bytes += b;
                count += 1;
                oldest = Some(oldest.map_or(created, |v: i64| v.min(created)));
                newest = Some(newest.map_or(created, |v: i64| v.max(created)));
            }
        }
        if count == 0 {
            continue;
        }
        out.push(CacheStatDto {
            peer_type: pt,
            peer_id: pid,
            name: if name.is_empty() { format!("{pid}") } else { name },
            bytes,
            count,
            oldest,
            newest,
        });
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    Ok(out)
}

pub fn overview(conn: &Connection, keep_days: i64, limit_bytes: i64) -> Result<CacheOverviewDto> {
    let (total_bytes, total_count) = total(conn)?;
    Ok(CacheOverviewDto {
        total_bytes,
        total_count,
        limit_bytes,
        keep_days,
        groups: stats_by_conversation(conn)?,
    })
}

/// 算出该淘汰哪些图（FR-42：先按保留天数，再按总量做 LRU）。
///
/// 只返回 sha256，删文件与更新消息状态由调用方做——`media` 与 `store` 各自管自己的事。
pub fn lru_candidates(conn: &Connection, keep_days: i64, limit_bytes: i64) -> Result<Vec<String>> {
    let now = now_ms();
    let mut victims: Vec<String> = Vec::new();

    // 1) 按 last_access_at 淘汰超过保留天数的
    if keep_days > 0 {
        let cutoff = now - keep_days * 86_400_000;
        let mut stmt = conn.prepare(
            "SELECT sha256 FROM image_cache WHERE last_access_at < ?1 ORDER BY last_access_at ASC",
        )?;
        let rows = stmt.query_map(params![cutoff], |r| r.get::<_, String>(0))?;
        for r in rows {
            victims.push(r?);
        }
    }

    // 2) 如果总量仍超上限，按 LRU 继续淘汰
    let (mut total_bytes, _) = total(conn)?;
    if limit_bytes > 0 && total_bytes > limit_bytes {
        let mut stmt = conn.prepare(
            "SELECT sha256, bytes FROM image_cache ORDER BY last_access_at ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            if total_bytes <= limit_bytes {
                break;
            }
            let (sha, bytes) = row?;
            if victims.contains(&sha) {
                continue;
            }
            total_bytes -= bytes;
            victims.push(sha);
        }
    }

    Ok(victims)
}

/// 删除索引行，并把引用该图的消息标记为"已清理"（FR-43：显示 `[图片已清除]`）
pub fn purge(conn: &Connection, shas: &[String]) -> Result<i64> {
    let mut freed = 0i64;
    for sha in shas {
        let bytes: i64 = conn
            .query_row(
                "SELECT bytes FROM image_cache WHERE sha256 = ?1",
                params![sha],
                |r| r.get(0),
            )
            .unwrap_or(0);
        conn.execute("DELETE FROM image_cache WHERE sha256 = ?1", params![sha])?;
        freed += bytes;

        // 消息里的图片段要改成"已清理"，否则前端会一直找一个不存在的文件
        let mut stmt = conn.prepare("SELECT message_id, segments FROM message WHERE has_image = 1")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut updates: Vec<(String, String)> = Vec::new();
        for row in rows {
            let (id, seg_json) = row?;
            let Ok(mut segs) = serde_json::from_str::<Vec<crate::model::Seg>>(&seg_json) else {
                continue;
            };
            let mut changed = false;
            for s in segs.iter_mut() {
                if let crate::model::Seg::Image { path, state, .. } = s {
                    if let Some(p) = path.clone() {
                        if crate::store::message::sha_from_path(&p).as_deref() == Some(sha.as_str()) {
                            *path = None;
                            *state = crate::model::image_state::CLEANED;
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                updates.push((id, serde_json::to_string(&segs)?));
            }
        }
        drop(stmt);
        for (id, json) in updates {
            conn.execute(
                "UPDATE message SET segments = ?2, image_state = ?3 WHERE message_id = ?1",
                params![id, json, crate::model::image_state::CLEANED],
            )?;
        }
    }
    Ok(freed)
}

/// 清理某个会话的全部图片（FR-42 的「按会话」）
pub fn shas_of_peer(conn: &Connection, peer: Peer) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT segments FROM message WHERE peer_type = ?1 AND peer_id = ?2 AND has_image = 1",
    )?;
    let rows = stmt.query_map(params![peer.peer_type, peer.peer_id], |r| {
        r.get::<_, String>(0)
    })?;
    let mut out: Vec<String> = Vec::new();
    for row in rows {
        let segs: Vec<crate::model::Seg> = serde_json::from_str(&row?).unwrap_or_default();
        for s in segs {
            if let crate::model::Seg::Image { path: Some(p), .. } = s {
                if let Some(sha) = crate::store::message::sha_from_path(&p) {
                    if !out.contains(&sha) {
                        out.push(sha);
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Seg;
    use crate::store::{conversation, message, Db};
    use anyhow::Result;

    fn sha(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn db_with_image() -> Db {
        let db = Db::open_memory().unwrap();
        let peer = Peer::group(30001);
        db.with(|c| conversation::ensure(c, peer, "大前端交流群", None)).unwrap();
        db.tx(|c| {
            register(c, &sha('a'), "aa/aa.png", "png", 1024, Some(100), Some(80), 0)?;
            message::upsert(
                c,
                &message::NewMessage {
                    message_id: "1".into(),
                    peer,
                    ts: 100,
                    sender_id: 1,
                    sender_name: None,
                    is_self: false,
                    segments: vec![Seg::Image {
                        path: Some(format!("C:/m/aa/{}.png", sha('a'))),
                        sub_type: 0,
                        state: crate::model::image_state::READY,
                    }],
                    reply_to: None,
                    is_at_me: false,
                    send_state: 0,
                },
            )?;
            Ok(())
        })
        .unwrap();
        db
    }

    #[test]
    fn 同图重复登记只存一份() -> Result<()> {
        let db = db_with_image();
        db.tx(|c| register(c, &sha('a'), "aa/aa.png", "png", 1024, Some(100), Some(80), 0))?;
        let (bytes, count) = db.with(|c| total(c)).unwrap();
        assert_eq!(count, 1);
        assert_eq!(bytes, 1024);
        Ok(())
    }

    #[test]
    fn 消息读取时补上图片引用() {
        let db = db_with_image();
        let m = db.with(|c| message::get(c, "1")).unwrap().unwrap();
        assert_eq!(m.images.len(), 1);
        assert_eq!(m.images[0].rel_path, "aa/aa.png");
        assert_eq!(m.images[0].width, Some(100));
        assert_eq!(m.has_image, true);
    }

    #[test]
    fn 按会话分组统计() {
        let db = db_with_image();
        let groups = db.with(|c| stats_by_conversation(c)).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].peer_id, 30001);
        assert_eq!(groups[0].count, 1);
        assert_eq!(groups[0].bytes, 1024);
    }

    #[test]
    fn 超过保留天数会被淘汰() {
        let db = db_with_image();
        // 手工把访问时间挪到 100 天前
        db.with(|c| {
            c.execute(
                "UPDATE image_cache SET last_access_at = ?1",
                params![crate::store::now_ms() - 100 * 86_400_000],
            )?;
            Ok(())
        })
        .unwrap();
        let victims = db.with(|c| lru_candidates(c, 30, 0)).unwrap();
        assert_eq!(victims, vec![sha('a')]);
    }

    #[test]
    fn 保留期内的图片不会被淘汰() {
        let db = db_with_image();
        assert!(db.with(|c| lru_candidates(c, 30, 0)).unwrap().is_empty());
    }

    #[test]
    fn 总量超上限时继续按_LRU_淘汰() {
        let db = db_with_image();
        // 上限压到 0，任何图都留不住
        let victims = db.with(|c| lru_candidates(c, 30, 1)).unwrap();
        assert_eq!(victims, vec![sha('a')]);
    }

    #[test]
    fn 清理后消息降级为已清除占位() {
        let db = db_with_image();
        let freed = db.tx(|c| purge(c, &[sha('a')])).unwrap();
        assert_eq!(freed, 1024);

        let m = db.with(|c| message::get(c, "1")).unwrap().unwrap();
        match &m.segments[0] {
            Seg::Image { path, state, .. } => {
                assert!(path.is_none());
                assert_eq!(*state, crate::model::image_state::CLEANED);
            }
            other => panic!("段类型错了: {other:?}"),
        }
        assert_eq!(m.images.len(), 0, "索引行已删，引用自然也补不上");
        let (bytes, count) = db.with(|c| total(c)).unwrap();
        assert_eq!((bytes, count), (0, 0));
    }

    #[test]
    fn 取会话的图片清单() {
        let db = db_with_image();
        let shas = db.with(|c| shas_of_peer(c, Peer::group(30001))).unwrap();
        assert_eq!(shas, vec![sha('a')]);
    }
}
