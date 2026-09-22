//! 账号归属的判定与重置（修复：换 QQ 账号后上个账号的数据残留在界面上）。
//!
//! **本库是单账号的**：`conversation` / `message` / `member_cache` / `mute` /
//! `tombstone` 都不带账号列。所以换账号时唯一正确的做法是把这些整体清空，
//! 再让新账号的会话从 NapCat 重新 seed（`ob::ws::resync`）。
//!
//! 刻意**不动 `settings`**：`ws_url` / `token` / 窗口位置 / 折叠条宽度这些是
//! 与账号无关的用户配置，换个号还得让用户重填就没有道理了。
//!
//! 这个模块是全项目唯一做"跨表清空"的地方。SQL 直接写在这里、而不是分散到各自的
//! store 模块，是为了让"哪些表属于账号"成为一份可读、可测的清单（`ACCOUNT_TABLES`），
//! 以后加表时不容易漏。

use anyhow::Result;
use rusqlite::Connection;

/// 每个表清掉多少行。只用于日志与自检。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResetReport {
    pub conversations: usize,
    pub messages: usize,
    pub members: usize,
    pub mutes: usize,
    pub tombstones: usize,
}

impl ResetReport {
    pub fn total(&self) -> usize {
        self.conversations + self.messages + self.members + self.mutes + self.tombstones
    }
}

/// 属于某个账号、换账号时必须清掉的表。加新表时别忘了往这里补。
pub const ACCOUNT_TABLES: &[&str] = &[
    "conversation",
    "message",
    "member_cache",
    "mute",
    "tombstone",
];

/// 清空所有与账号绑定的数据。表名写死在这里，不走拼接。
pub fn reset(conn: &Connection) -> Result<ResetReport> {
    Ok(ResetReport {
        conversations: conn.execute("DELETE FROM conversation", [])?,
        messages: conn.execute("DELETE FROM message", [])?,
        members: conn.execute("DELETE FROM member_cache", [])?,
        mutes: conn.execute("DELETE FROM mute", [])?,
        tombstones: conn.execute("DELETE FROM tombstone", [])?,
    })
}

/// 本地是否已经有会话/消息数据。用来处理"升级前从没记过 self_id"这一种情况。
pub fn has_local_data(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT (EXISTS(SELECT 1 FROM conversation) OR EXISTS(SELECT 1 FROM message))",
        [],
        |r| r.get(0),
    )?;
    Ok(n != 0)
}

/// 这次登录是否意味着"换了个账号"。
///
/// * `prev` —— meta 里记录的 self_id，`0` 表示从没记录过（老版本从未写过这个键）；
/// * `has_local_data` —— 本地是否已有会话/消息数据。
///
/// 有记录时按"值不同即切换"；**没记录时，若本地已有数据，其归属无从考证**，
/// 一律当作上个账号的残留清掉。理由：清掉的数据 NapCat 侧还能重新 seed / 重新拉取，
/// 而残留在界面上的错数据会一直误导用户（正是这次要修的 bug）。
/// 全新安装时 `has_local_data` 为 false，不会误清。
pub fn account_switched(prev: i64, has_local_data: bool, next: i64) -> bool {
    if next <= 0 {
        return false;
    }
    if prev > 0 {
        return prev != next;
    }
    has_local_data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::schema;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    fn seed_rows(conn: &Connection) {
        conn.execute(
            "INSERT INTO conversation(peer_type, peer_id, name, unread_count, has_mention,
                                      is_manual_tab, updated_at)
             VALUES(1, 100, '群', 0, 0, 0, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message(message_id, peer_type, peer_id, ts, sender_id, segments,
                                 seg_types, created_at)
             VALUES('m1', 1, 100, 1, 2, '[]', 'text', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO member_cache(group_id, user_id, nickname, updated_at)
             VALUES(100, 2, '小李', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mute(peer_type, peer_id, muted_at) VALUES(1, 100, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tombstone(message_id, peer_type, peer_id, deleted_at)
             VALUES('t1', 1, 100, 1)",
            [],
        )
        .unwrap();
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn 同账号不切换() {
        assert!(!account_switched(1001, true, 1001));
        assert!(!account_switched(1001, false, 1001));
    }

    #[test]
    fn 不同账号判定为切换() {
        assert!(account_switched(1001, false, 1002));
        assert!(account_switched(1001, true, 1002));
    }

    #[test]
    fn 没记过self_id时看本地有没有数据() {
        // 升级场景：本地有上一个账号的数据，归属未知 → 当作切换，清掉
        assert!(account_switched(0, true, 1002));
        // 全新安装：什么都没，不算切换
        assert!(!account_switched(0, false, 1002));
    }

    #[test]
    fn 拿不到self_id时绝不动作() {
        assert!(!account_switched(1001, true, 0));
        assert!(!account_switched(1001, true, -1));
    }

    #[test]
    fn reset清空账号数据但保留设置() {
        let conn = mem();
        seed_rows(&conn);
        crate::store::settings::set(&conn, "ws_url", &serde_json::json!("ws://10.0.0.5:3999"))
            .unwrap();
        schema::meta_set(&conn, "self_id", "1001").unwrap();

        let r = reset(&conn).unwrap();
        assert_eq!(
            r,
            ResetReport {
                conversations: 1,
                messages: 1,
                members: 1,
                mutes: 1,
                tombstones: 1
            }
        );
        assert_eq!(r.total(), 5);

        for t in ACCOUNT_TABLES {
            assert_eq!(count(&conn, t), 0, "{t} 必须被清空");
        }

        // 设置是与账号无关的用户配置，绝不能被清掉
        // （用一个和默认值不同的地址，否则"没被清掉"和"回退到默认值"分不出来）
        assert_eq!(
            crate::store::settings::get_all(&conn).unwrap().ws_url,
            "ws://10.0.0.5:3999",
            "换账号不该让用户重填连接配置"
        );
        assert_eq!(
            schema::meta_get(&conn, "self_id").unwrap().as_deref(),
            Some("1001"),
            "meta 不是账号数据，reset 不该碰它"
        );
    }

    #[test]
    fn reset对空库是幂等的() {
        let conn = mem();
        assert_eq!(reset(&conn).unwrap(), ResetReport::default());
        assert_eq!(reset(&conn).unwrap().total(), 0);
    }

    #[test]
    fn has_local_data_认会话也认消息() {
        let conn = mem();
        assert!(!has_local_data(&conn).unwrap(), "空库不该算有数据");

        conn.execute(
            "INSERT INTO conversation(peer_type, peer_id, name, unread_count, has_mention,
                                      is_manual_tab, updated_at)
             VALUES(0, 9, '甲', 0, 0, 0, 1)",
            [],
        )
        .unwrap();
        assert!(has_local_data(&conn).unwrap(), "有会话就算有数据");

        reset(&conn).unwrap();
        assert!(!has_local_data(&conn).unwrap());

        // 只有消息、没有会话行，也算有数据
        conn.execute(
            "INSERT INTO message(message_id, peer_type, peer_id, ts, sender_id, segments,
                                 seg_types, created_at)
             VALUES('x', 0, 9, 1, 2, '[]', 'text', 1)",
            [],
        )
        .unwrap();
        assert!(has_local_data(&conn).unwrap(), "有消息就算有数据");
    }

    #[test]
    fn 清单里的表都真实存在() {
        let conn = mem();
        for t in ACCOUNT_TABLES {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "清单里的 {t} 在建表语句里找不到，改名后要同步");
        }
    }
}
