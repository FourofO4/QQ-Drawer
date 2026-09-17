//! 上游报文的宽容读取工具。
//!
//! 这里只做"从 Value 里安全地抠字段"这一件事。刻意不写严格的结构体：
//! 上游字段缺失、类型漂移（数字 vs 字符串）在真实环境里都会遇到，
//! 一次 `.as_i64()` 拿不到就退一步 `.as_str().parse()` 是最实用的写法。

use serde_json::Value;

/// 数字字段。上游有时把 id 序列化成字符串，所以两种都认。
pub fn as_i64(v: &Value, key: &str) -> Option<i64> {
    let raw = v.get(key)?;
    if let Some(n) = raw.as_i64() {
        return Some(n);
    }
    if let Some(f) = raw.as_f64() {
        return Some(f as i64);
    }
    raw.as_str()?.trim().parse().ok()
}

/// 字符串字段。上游偶尔把数字塞进字符串字段，也一并接住。
pub fn as_str(v: &Value, key: &str) -> Option<String> {
    let raw = v.get(key)?;
    if let Some(s) = raw.as_str() {
        return Some(s.to_string());
    }
    if raw.is_number() {
        return Some(raw.to_string());
    }
    None
}

pub fn as_bool(v: &Value, key: &str) -> Option<bool> {
    let raw = v.get(key)?;
    if let Some(b) = raw.as_bool() {
        return Some(b);
    }
    match raw.as_i64() {
        Some(0) => Some(false),
        Some(_) => Some(true),
        None => None,
    }
}

/// 数组字段，拿不到就返回空数组（调用方少写一堆 `unwrap_or_default`）。
pub fn as_array(v: &Value, key: &str) -> Vec<Value> {
    v.get(key)
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default()
}

/// 深层取值：`get_path(v, &["data", "sender", "nickname"])`
pub fn get_path<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    Some(cur)
}

pub fn deep_str(v: &Value, path: &[&str]) -> Option<String> {
    let raw = get_path(v, path)?;
    raw.as_str().map(|s| s.to_string())
}

pub fn deep_i64(v: &Value, path: &[&str]) -> Option<i64> {
    let raw = get_path(v, path)?;
    if let Some(n) = raw.as_i64() {
        Some(n)
    } else {
        raw.as_str()?.trim().parse().ok()
    }
}

/// 时间戳归一化：上游有的给秒，有的给毫秒。
pub fn normalize_ts(raw: i64) -> i64 {
    if raw > 0 && raw < 100_000_000_000 {
        raw * 1000 // 秒 → 毫秒
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 数字字段_兼容字符串形态() {
        let v = json!({ "a": 123, "b": "456", "c": 1.0, "d": "abc" });
        assert_eq!(as_i64(&v, "a"), Some(123));
        assert_eq!(as_i64(&v, "b"), Some(456));
        assert_eq!(as_i64(&v, "c"), Some(1));
        assert_eq!(as_i64(&v, "d"), None);
        assert_eq!(as_i64(&v, "missing"), None);
    }

    #[test]
    fn 字符串字段_兼容数字形态() {
        let v = json!({ "name": "李工", "qq": 30011 });
        assert_eq!(as_str(&v, "name").as_deref(), Some("李工"));
        assert_eq!(as_str(&v, "qq").as_deref(), Some("30011"));
        assert_eq!(as_str(&v, "missing"), None);
    }

    #[test]
    fn 布尔字段_兼容零与非零() {
        let v = json!({ "t": true, "f": false, "one": 1, "zero": 0 });
        assert_eq!(as_bool(&v, "t"), Some(true));
        assert_eq!(as_bool(&v, "f"), Some(false));
        assert_eq!(as_bool(&v, "one"), Some(true));
        assert_eq!(as_bool(&v, "zero"), Some(false));
        assert_eq!(as_bool(&v, "missing"), None);
    }

    #[test]
    fn 数组字段_拿不到就是空() {
        let v = json!({ "arr": [1, 2], "not_arr": 5 });
        assert_eq!(as_array(&v, "arr").len(), 2);
        assert!(as_array(&v, "not_arr").is_empty());
        assert!(as_array(&v, "missing").is_empty());
    }

    #[test]
    fn 深层取值() {
        let v = json!({ "data": { "sender": { "nickname": "李工", "user_id": 30011 } } });
        assert_eq!(deep_str(&v, &["data", "sender", "nickname"]).as_deref(), Some("李工"));
        assert_eq!(deep_i64(&v, &["data", "sender", "user_id"]), Some(30011));
        assert!(deep_str(&v, &["data", "nope", "x"]).is_none());
    }

    #[test]
    fn 时间戳归一化_秒补成毫秒() {
        assert_eq!(normalize_ts(1_700_000_000), 1_700_000_000_000);
        assert_eq!(normalize_ts(1_700_000_000_000), 1_700_000_000_000);
        assert_eq!(normalize_ts(0), 0);
    }
}
