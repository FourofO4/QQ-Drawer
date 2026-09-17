//! 表结构与迁移（§4.4）。
//!
//! 三条不能动的前提：
//!  1. `message.message_id` 是主键 —— 幂等的地基（FR-22）；
//!  2. 所有读取路径都必须按 `tombstone` 过滤，否则删掉的消息会被远端补齐"送回来"；
//!  3. 本库是消息的**唯一长期副本**，清理 = 永久丢失。

use anyhow::Result;
use rusqlite::Connection;

/// 代码期望的表结构版本。改动 DDL 时 +1，并在 `migrate` 里补一段顺序迁移。
pub const SCHEMA_VERSION: i64 = 1;

const DDL: &str = r#"
-- 会话
CREATE TABLE IF NOT EXISTS conversation (
    peer_type       INTEGER NOT NULL,          -- 0=私聊 1=群聊
    peer_id         INTEGER NOT NULL,          -- user_id 或 group_id
    name            TEXT    NOT NULL,          -- 展示名：备注 > 群名片 > 昵称/群名
    raw_name        TEXT,
    remark          TEXT,
    last_msg_time   INTEGER,
    last_msg_text   TEXT,                      -- 折叠条预览（已单行化与截断）
    last_msg_sender TEXT,
    unread_count    INTEGER NOT NULL DEFAULT 0,
    has_mention     INTEGER NOT NULL DEFAULT 0,
    tab_order       INTEGER,                   -- NULL=不在标签栏
    is_manual_tab   INTEGER NOT NULL DEFAULT 0,
    updated_at      INTEGER NOT NULL,
    PRIMARY KEY (peer_type, peer_id)
);
CREATE INDEX IF NOT EXISTS idx_conv_recent ON conversation(last_msg_time DESC);
CREATE INDEX IF NOT EXISTS idx_conv_tab    ON conversation(tab_order) WHERE tab_order IS NOT NULL;

-- 消息
CREATE TABLE IF NOT EXISTS message (
    message_id      TEXT PRIMARY KEY,
    peer_type       INTEGER NOT NULL,
    peer_id         INTEGER NOT NULL,
    seq             INTEGER,
    ts              INTEGER NOT NULL,
    sender_id       INTEGER NOT NULL,
    sender_name     TEXT,
    is_self         INTEGER NOT NULL DEFAULT 0,
    text            TEXT,
    segments        TEXT NOT NULL,
    seg_types       TEXT NOT NULL,
    reply_to        TEXT,
    is_at_me        INTEGER NOT NULL DEFAULT 0,
    has_image       INTEGER NOT NULL DEFAULT 0,
    image_state     INTEGER NOT NULL DEFAULT 0,
    is_recalled     INTEGER NOT NULL DEFAULT 0,
    recalled_by     INTEGER,
    recalled_at     INTEGER,
    send_state      INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_msg_id       ON message(message_id);
CREATE INDEX        IF NOT EXISTS idx_msg_peer_time ON message(peer_type, peer_id, ts DESC);
CREATE INDEX        IF NOT EXISTS idx_msg_atme      ON message(peer_type, peer_id, is_at_me) WHERE is_at_me = 1;

-- 图片索引（同时是 LRU 依据）
CREATE TABLE IF NOT EXISTS image_cache (
    sha256          TEXT PRIMARY KEY,
    rel_path        TEXT NOT NULL,
    ext             TEXT NOT NULL,
    bytes           INTEGER NOT NULL,
    width           INTEGER,
    height          INTEGER,
    sub_type        INTEGER NOT NULL DEFAULT 0,
    ref_count       INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    last_access_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_img_lru ON image_cache(last_access_at);

-- 群成员（@ 选人与名字解析）
CREATE TABLE IF NOT EXISTS member_cache (
    group_id    INTEGER NOT NULL,
    user_id     INTEGER NOT NULL,
    nickname    TEXT,
    card        TEXT,
    role        TEXT,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (group_id, user_id)
);

-- 本地静音表
CREATE TABLE IF NOT EXISTS mute (
    peer_type  INTEGER NOT NULL,
    peer_id    INTEGER NOT NULL,
    muted_at   INTEGER NOT NULL,
    reason     TEXT,
    PRIMARY KEY (peer_type, peer_id)
);

-- 墓碑（手动删除记录）
CREATE TABLE IF NOT EXISTS tombstone (
    message_id  TEXT PRIMARY KEY,
    peer_type   INTEGER NOT NULL,
    peer_id     INTEGER NOT NULL,
    deleted_at  INTEGER NOT NULL,
    note        TEXT
);

-- 配置
CREATE TABLE IF NOT EXISTS settings (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL,                      -- JSON 编码
    updated_at INTEGER NOT NULL
);

-- 元信息：schema_version / self_id / last_connect_ok / napcat_version
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
"#;

/// 建表 + 版本迁移。幂等，可以每次启动都跑。
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(DDL)?;

    let current: i64 = meta_get(conn, "schema_version")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    if current == 0 {
        // 全新库：直接盖上当前版本
        meta_set(conn, "schema_version", &SCHEMA_VERSION.to_string())?;
    } else if current < SCHEMA_VERSION {
        // 后续版本在这里逐级迁移：每个 if 只负责升一级，全部在一个事务里由调用方保证
        // if current < 2 { ... }
        meta_set(conn, "schema_version", &SCHEMA_VERSION.to_string())?;
    }
    Ok(())
}

pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

/// 迁移前的备份（§4.10：把 data.db 复制成 data.db.bak.<version>，保留最近 3 份）
pub fn backup_before_migrate(db_path: &std::path::Path, version: i64) -> Result<()> {
    if !db_path.exists() {
        return Ok(());
    }
    let bak = db_path.with_extension(format!("db.bak.{version}"));
    std::fs::copy(db_path, &bak)?;

    // 只保留最近 3 份
    let dir = db_path.parent().unwrap_or(std::path::Path::new("."));
    let prefix = format!(
        "{}.bak.",
        db_path.file_name().and_then(|s| s.to_str()).unwrap_or("data.db")
    );
    let mut baks: Vec<std::path::PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    baks.sort();
    while baks.len() > 3 {
        let old = baks.remove(0);
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = mem();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(
            meta_get(&conn, "schema_version").unwrap().unwrap(),
            SCHEMA_VERSION.to_string()
        );
    }

    #[test]
    fn all_tables_created() {
        let conn = mem();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for t in [
            "conversation",
            "image_cache",
            "member_cache",
            "message",
            "meta",
            "mute",
            "settings",
            "tombstone",
        ] {
            assert!(names.iter().any(|n| n == t), "缺表 {t}");
        }
    }

    #[test]
    fn wal_and_sync_pragmas_applied() {
        let conn = mem();
        // 内存库的 journal_mode 会是 memory，只验证设置语句没有报错且同步级别生效
        let sync: String = conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sync, "1"); // NORMAL
    }

    #[test]
    fn message_id_is_primary_key() {
        let conn = mem();
        conn.execute(
            "INSERT INTO message(message_id, peer_type, peer_id, ts, sender_id, segments, seg_types, created_at)
             VALUES('1', 1, 2, 3, 4, '[]', 'text', 5)",
            [],
        )
        .unwrap();
        let dup = conn.execute(
            "INSERT INTO message(message_id, peer_type, peer_id, ts, sender_id, segments, seg_types, created_at)
             VALUES('1', 1, 2, 3, 4, '[]', 'text', 5)",
            [],
        );
        assert!(dup.is_err(), "message_id 必须是主键，否则幂等无从谈起");
    }
}
