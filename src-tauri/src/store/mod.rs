//! SQLite 持久层（§4.1 单一数据源）。
//!
//! 全项目只有这里有写权限：前端不缓存业务状态，`ob` 层不做落盘，
//! 窗口层不碰数据。所有写入走事务（NFR-08：异常退出不丢已落盘数据）。
//!
//! 连接用 `parking_lot::Mutex` 而不是 `std::sync::Mutex`：这里不存在跨 await 持锁的写法，
//! 也不需要处理中毒状态，出错的路径全都返回 `Result` 自己兜。

pub mod account;
pub mod conversation;
pub mod image;
pub mod member;
pub mod message;
pub mod mute;
pub mod schema;
pub mod settings;
pub mod tombstone;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{Connection, Transaction};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub struct Db {
    inner: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl Db {
    /// 打开（必要时创建）数据库。启动流程里调用一次。
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建数据目录失败: {}", dir.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("打开数据库失败: {}", path.display()))?;
        schema::migrate(&conn)?;
        Ok(Self { inner: Arc::new(Mutex::new(conn)), path: path.to_path_buf() })
    }

    /// 内存库：只给单测用。
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::migrate(&conn)?;
        Ok(Self { inner: Arc::new(Mutex::new(conn)), path: PathBuf::from(":memory:") })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 只读（或单条写入）访问。写成 `with` 而不是暴露连接，是为了让"连接是共享的"这件事显式。
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.inner.lock();
        f(&guard)
    }

    /// 事务访问。批量写入一律走这里。
    pub fn tx<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut guard = self.inner.lock();
        let tx = guard.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

/// 当前时间戳（毫秒）。全项目统一走这里，别散落 `SystemTime::now()`。
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// SQLite 没有布尔类型，统一用 0/1 存。
pub(crate) fn b2i(v: bool) -> i64 {
    if v {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_memory_works() {
        let db = Db::open_memory().unwrap();
        let n: i64 = db.with(|c| Ok(c.query_row("SELECT 1", [], |r| r.get(0))?)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn tx_rolls_back_on_error() {
        let db = Db::open_memory().unwrap();
        let r: Result<()> = db.tx(|tx| {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES('a', '1')",
                [],
            )?;
            anyhow::bail!("boom");
        });
        assert!(r.is_err());
        let exists = db
            .with(|c| schema::meta_get(c, "a"))
            .unwrap();
        assert!(exists.is_none(), "事务出错必须回滚");
    }

    #[test]
    fn now_ms_is_reasonable() {
        // 2026-01-01 之后、2100 之前
        let t = now_ms();
        assert!(t > 1_767_225_600_000);
        assert!(t < 4_102_444_800_000);
    }
}
