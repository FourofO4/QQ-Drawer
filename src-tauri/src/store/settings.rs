//! 设置持久化（FR-47）。
//!
//! 表结构是通用的 key/value（value 存 JSON），但读写都经过 `Settings` 结构体：
//!  · 读：以 `Settings::default()` 为底，把库里有的键盖上去 —— 加字段不用写迁移，
//!    老库缺新键也不会炸；
//!  · 写：前端只说改了哪个键（`set_setting(key, value)`），这里按字段名逐个落库。

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use serde_json::Value;

use crate::model::Settings;
use crate::store::now_ms;

pub fn get_all(conn: &Connection) -> Result<Settings> {
    let mut base = serde_json::to_value(Settings::default())?;
    let map = base
        .as_object_mut()
        .ok_or_else(|| anyhow!("Settings 必须是对象"))?;

    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (k, v) = row?;
        if let Ok(parsed) = serde_json::from_str::<Value>(&v) {
            map.insert(k, parsed);
        }
    }
    Ok(serde_json::from_value(base)?)
}

/// 只更新一个键。未知的键直接忽略——前端多传一个字段不该让程序崩。
pub fn set(conn: &Connection, key: &str, value: &Value) -> Result<()> {
    let known = serde_json::to_value(Settings::default())?;
    if known.get(key).is_none() {
        tracing::warn!(key, "忽略未知的设置项");
        return Ok(());
    }
    conn.execute(
        "INSERT INTO settings(key, value, updated_at) VALUES(?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, serde_json::to_string(value)?, now_ms()],
    )?;
    tracing::debug!(key, "设置已更新");
    Ok(())
}

/// 清空全部本地数据时用（设置本身也回到默认）
pub fn reset(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM settings", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;
    use serde_json::json;

    #[test]
    fn 空库读出来是默认值() {
        let db = Db::open_memory().unwrap();
        let s = db.with(get_all).unwrap();
        assert_eq!(s.ws_url, "ws://127.0.0.1:3001");
        assert_eq!(s.bar_width, 264);
        assert_eq!(s.panel_alpha, 0.72);
        assert_eq!(s.tab_limit, 5);
        assert!(s.always_on_top);
    }

    #[test]
    fn 写一个键只影响那个键() {
        let db = Db::open_memory().unwrap();
        db.tx(|c| set(c, "bar_width", &json!(320))).unwrap();
        let s = db.with(get_all).unwrap();
        assert_eq!(s.bar_width, 320);
        assert_eq!(s.panel_alpha, 0.72, "别的字段必须保持默认");
    }

    #[test]
    fn 设置重启后还在() {
        let db = Db::open_memory().unwrap();
        db.tx(|c| set(c, "access_token", &json!("secret-token"))).unwrap();
        db.tx(|c| set(c, "locked", &json!(true))).unwrap();
        let s = db.with(get_all).unwrap();
        assert_eq!(s.access_token, "secret-token");
        assert!(s.locked);
    }

    #[test]
    fn 未知键被忽略而不是报错() {
        let db = Db::open_memory().unwrap();
        db.tx(|c| set(c, "不存在的设置", &json!(1))).unwrap();
        let n: i64 = db
            .with(|c| Ok(c.query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn 老库缺新键也能读() {
        let db = Db::open_memory().unwrap();
        // 手工塞一个只属于"老版本"的键
        db.with(|c| {
            c.execute(
                "INSERT INTO settings(key, value, updated_at) VALUES('ws_url', '\"ws://old\"', 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let s = db.with(get_all).unwrap();
        assert_eq!(s.ws_url, "ws://old");
        assert_eq!(s.bubble_alpha, 0.35, "缺失的键回落到默认值");
    }

    #[test]
    fn 窗口位置可以持久化() {
        let db = Db::open_memory().unwrap();
        db.tx(|c| {
            set(c, "window_x", &json!(1600))?;
            set(c, "window_y", &json!(40))?;
            Ok(())
        })
        .unwrap();
        let s = db.with(get_all).unwrap();
        assert_eq!(s.window_x, Some(1600));
        assert_eq!(s.window_y, Some(40));
    }

    #[test]
    fn 重置设置回到默认() {
        let db = Db::open_memory().unwrap();
        db.tx(|c| set(c, "bar_width", &json!(400))).unwrap();
        db.tx(|c| reset(c)).unwrap();
        assert_eq!(db.with(get_all).unwrap().bar_width, 264);
    }
}
