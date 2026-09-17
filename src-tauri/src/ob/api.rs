//! OneBot action 封装（§4.6 接口映射全集）。
//!
//! **为什么要有 `ActionBus`：** OneBot 11 的 WebSocket 是全双工的，发出去的 action
//! 与回来的响应靠 `echo` 字段配对。直接在调用点 `write().await` 会让「发」与「收」
//! 耦合在一起，而且没法给单个 action 设超时。这里把它拆成两半：
//!
//! ```text
//!   cmd.rs ──call()──▶ ActionBus ──mpsc──▶ ws 写半边 ──▶ NapCat
//!                          ▲                              │
//!                          └── resolve() ◀── ws 读半边 ◀───┘
//! ```
//!
//! `call()` 在 `pending` 表里登记一个 oneshot 发送端，然后等它；`ws` 读到一个带 `echo`
//! 的报文就交给 `resolve()` 唤醒对应的等待者。所以**每个 action 都能独立超时**，
//! 一条卡住的 `get_group_member_list` 不会拖死发送消息。
//!
//! 帧构造与响应解析都是纯函数（`build_frame` / `extract_response` / `items_of`），带单测。

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// 单个 action 的默认超时。
///
/// 本机直连（抽屉与 NapCat 同进程机器），8 秒已经远超正常往返；
/// 拉历史与成员列表这类重活单独放宽（见 `HEAVY_TIMEOUT_MS`）。
pub const DEFAULT_TIMEOUT_MS: u64 = 8_000;
/// 历史分页 / 成员列表 / 图片直链这类可能涉及磁盘与外网的 action。
pub const HEAVY_TIMEOUT_MS: u64 = 20_000;

/* ------------------------------ 纯函数：帧与响应 ------------------------------ */

/// 构造发给 NapCat 的 action 帧。`echo` 由调用方给，便于测试固定值。
pub fn build_frame(action: &str, params: Value, echo: &str) -> Value {
    json!({ "action": action, "params": params, "echo": echo })
}

/// 解析后的响应。
#[derive(Clone, Debug, PartialEq)]
pub struct ActionResponse {
    pub echo: Option<String>,
    /// `retcode == 0`（或 `status == "ok"`）才算成功
    pub ok: bool,
    /// 失败原因，直接可以给用户看
    pub error: Option<String>,
    pub data: Value,
}

impl ActionResponse {
    pub fn empty() -> Self {
        Self { echo: None, ok: false, error: None, data: Value::Null }
    }

    /// 取 `data` 里的一个数组字段。上游对同一份数据有多种包裹方式，这里一次都认掉。
    pub fn items(&self, keys: &[&str]) -> Vec<Value> {
        items_of(&self.data, keys)
    }
}

/// 从上游报文里挑一个数组出来。
///
/// `data` 本身可能是数组（`get_recent_contact`），也可能是 `{"messages": [...]}`、
/// `{"list": [...]}`（不同版本、不同接口的差异），所以按候选键依次试。
pub fn items_of(data: &Value, keys: &[&str]) -> Vec<Value> {
    if let Some(arr) = data.as_array() {
        return arr.clone();
    }
    for k in keys {
        if let Some(arr) = data.get(*k).and_then(|v| v.as_array()) {
            return arr.clone();
        }
    }
    Vec::new()
}

/// 解析上游响应。**宽容优先**：字段缺失时按"失败"处理，但绝不 panic。
pub fn extract_response(v: &Value) -> ActionResponse {
    // echo 可能是字符串或数字，统一成字符串（自己发出去的一定是字符串）
    let echo = match v.get("echo") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };

    let retcode = v.get("retcode").and_then(|x| x.as_i64());
    let status = v.get("status").and_then(|x| x.as_str());
    let ok = match (retcode, status) {
        (Some(code), _) => code == 0,
        (None, Some(s)) => s == "ok",
        // 既没有 retcode 也没有 status：当成事件/心跳，不算 action 响应
        (None, None) => false,
    };

    let error = if ok {
        None
    } else {
        v.get("message")
            .or_else(|| v.get("wording"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .or_else(|| retcode.map(|c| format!("上游返回 retcode={c}")))
            .or_else(|| Some("上游未返回 retcode".to_string()))
    };

    ActionResponse {
        echo,
        ok,
        error,
        data: v.get("data").cloned().unwrap_or(Value::Null),
    }
}

/// 从 `send_*_msg` 的响应里取 `message_id`。
pub fn sent_message_id(resp: &ActionResponse) -> Option<String> {
    let raw = resp.data.get("message_id")?;
    if let Some(s) = raw.as_str() {
        return Some(s.to_string());
    }
    raw.as_i64().map(|n| n.to_string())
}

/// 是不是一个 action 响应（而不是事件推送）。
///
/// 判定依据只看 `echo`：事件推送永远不带它。不靠 `post_type` 缺失来判断，
/// 因为某些版本的响应里也带 `post_type`。
pub fn is_response(v: &Value) -> bool {
    v.get("echo").is_some()
}

/* ------------------------------ 总线 ------------------------------ */

struct Inner {
    tx: mpsc::UnboundedSender<Value>,
    pending: Mutex<HashMap<String, oneshot::Sender<ActionResponse>>>,
    echo: AtomicU64,
}

#[derive(Clone)]
pub struct ActionBus {
    inner: Arc<Inner>,
}

impl ActionBus {
    /// `tx` 是发往 ws 写半边的通道。
    pub fn new(tx: mpsc::UnboundedSender<Value>) -> Self {
        Self {
            inner: Arc::new(Inner {
                tx,
                pending: Mutex::new(HashMap::new()),
                echo: AtomicU64::new(1),
            }),
        }
    }

    /// 建一对「总线 + 出站帧接收端」。生产代码里接收端由 ws 任务持有；单测里直接读它。
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<Value>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self::new(tx), rx)
    }

    /// 连接断开时清空等待者，让所有在途的 `call()` 立刻失败而不是空等到超时。
    pub fn fail_all(&self, reason: &str) {
        let mut map = self.inner.pending.lock();
        let count = map.len();
        for (_, tx) in map.drain() {
            let _ = tx.send(ActionResponse {
                echo: None,
                ok: false,
                error: Some(reason.to_string()),
                data: Value::Null,
            });
        }
        if count > 0 {
            tracing::debug!(count, "连接断开，作废在途 action");
        }
    }

    /// ws 读半边拿到带 `echo` 的报文时调用。返回 `true` 表示确实有人等它。
    pub fn resolve(&self, resp: ActionResponse) -> bool {
        let Some(echo) = resp.echo.clone() else {
            return false;
        };
        let waiter = self.inner.pending.lock().remove(&echo);
        match waiter {
            Some(tx) => {
                // 接收端可能已经超时走了，send 失败不算错误
                let _ = tx.send(resp);
                true
            }
            None => {
                tracing::debug!(echo, "收到无人认领的 action 响应（可能已超时）");
                false
            }
        }
    }

    /// 发一个 action 并等响应。这是所有接口封装的唯一实现。
    pub async fn call(&self, action: &str, params: Value) -> Result<Value> {
        self.call_with_timeout(action, params, DEFAULT_TIMEOUT_MS).await
    }

    pub async fn call_heavy(&self, action: &str, params: Value) -> Result<Value> {
        self.call_with_timeout(action, params, HEAVY_TIMEOUT_MS).await
    }

    pub async fn call_with_timeout(
        &self,
        action: &str,
        params: Value,
        timeout_ms: u64,
    ) -> Result<Value> {
        let echo = self.inner.echo.fetch_add(1, Ordering::Relaxed).to_string();
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().insert(echo.clone(), tx);

        let frame = build_frame(action, params, &echo);
        if self.inner.tx.send(frame).is_err() {
            self.inner.pending.lock().remove(&echo);
            anyhow::bail!("连接已关闭，无法发送 {action}");
        }

        let resp = match tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            rx,
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => {
                // 发送端被丢弃（fail_all 或 ws 任务退出）
                self.inner.pending.lock().remove(&echo);
                anyhow::bail!("{action} 被中断：连接已断开");
            }
            Err(_) => {
                self.inner.pending.lock().remove(&echo);
                anyhow::bail!("{action} 超时（{timeout_ms} ms）");
            }
        };

        if resp.ok {
            Ok(resp.data)
        } else {
            Err(anyhow!(
                "{action} 失败：{}",
                resp.error.unwrap_or_else(|| "上游未说明原因".into())
            ))
        }
    }

    /// 不需要结果的动作（标记已读之类）。失败只记日志——这些动作失败不该打扰用户。
    pub fn fire(&self, action: &str, params: Value) {
        let echo = self.inner.echo.fetch_add(1, Ordering::Relaxed).to_string();
        let frame = build_frame(action, params, &echo);
        if self.inner.tx.send(frame).is_err() {
            tracing::debug!(action, "连接已关闭，丢弃动作");
        }
    }

    pub fn is_alive(&self) -> bool {
        !self.inner.tx.is_closed()
    }
}

/* ------------------------------ 具体接口（§4.6） ------------------------------ */

impl ActionBus {
    /* --- 连接与状态 --- */

    /// 连接成功后第一件事：拿 `self_id`。
    pub async fn get_login_info(&self) -> Result<Value> {
        self.call("get_login_info", json!({})).await
    }

    /// 记录 NapCat 版本，排查兼容问题时很有用。
    pub async fn get_version_info(&self) -> Result<Value> {
        self.call("get_version_info", json!({})).await
    }

    pub async fn get_status(&self) -> Result<Value> {
        self.call("get_status", json!({})).await
    }

    /// NapCat 自身缓存清理（可选入口，需提示影响）。
    pub async fn clean_cache(&self) -> Result<Value> {
        self.call_heavy("clean_cache", json!({})).await
    }

    /* --- 会话与历史 --- */

    /// 会话列表种子。返回字段在不同版本间有两种形态，由 `ob::segments` 兼容。
    pub async fn get_recent_contact(&self, count: i64) -> Result<Value> {
        self.call_heavy("get_recent_contact", json!({ "count": count })).await
    }

    pub async fn get_friend_list(&self) -> Result<Value> {
        self.call_heavy("get_friend_list", json!({ "no_cache": false })).await
    }

    pub async fn get_group_list(&self) -> Result<Value> {
        self.call_heavy("get_group_list", json!({ "no_cache": false })).await
    }

    pub async fn get_group_info(&self, group_id: i64) -> Result<Value> {
        self.call("get_group_info", json!({ "group_id": group_id, "no_cache": false })).await
    }

    pub async fn get_group_member_list(&self, group_id: i64) -> Result<Value> {
        self.call_heavy(
            "get_group_member_list",
            json!({ "group_id": group_id, "no_cache": false }),
        )
        .await
    }

    pub async fn get_group_member_info(&self, group_id: i64, user_id: i64) -> Result<Value> {
        self.call(
            "get_group_member_info",
            json!({ "group_id": group_id, "user_id": user_id, "no_cache": false }),
        )
        .await
    }

    /// `message_seq = 0` 表示「最新一页」。
    pub async fn get_group_msg_history(
        &self,
        group_id: i64,
        message_seq: i64,
        count: i64,
    ) -> Result<Value> {
        self.call_heavy(
            "get_group_msg_history",
            json!({
                "group_id": group_id,
                "message_seq": message_seq,
                "count": count,
                "reverse_order": false,
                "disable_get_url": false,
                "parse_mult_msg": false
            }),
        )
        .await
    }

    pub async fn get_friend_msg_history(
        &self,
        user_id: i64,
        message_seq: i64,
        count: i64,
    ) -> Result<Value> {
        self.call_heavy(
            "get_friend_msg_history",
            json!({
                "user_id": user_id,
                "message_seq": message_seq,
                "count": count,
                "reverse_order": false,
                "disable_get_url": false,
                "parse_mult_msg": false
            }),
        )
        .await
    }

    /// 单条消息详情（引用解析兜底）。受 LRU 限制，可能已经拿不到了。
    pub async fn get_msg(&self, message_id: &str) -> Result<Value> {
        self.call("get_msg", json!({ "message_id": message_id })).await
    }

    /// 合并转发内容（MVP 只渲染占位，不展开）。
    pub async fn get_forward_msg(&self, message_id: &str) -> Result<Value> {
        self.call_heavy("get_forward_msg", json!({ "message_id": message_id })).await
    }

    /* --- 发送 --- */

    /// 消息段数组由 `ob::segments::build_outgoing` 产出。
    pub async fn send_group_msg(&self, group_id: i64, message: Value) -> Result<Value> {
        self.call("send_group_msg", json!({ "group_id": group_id, "message": message })).await
    }

    pub async fn send_private_msg(&self, user_id: i64, message: Value) -> Result<Value> {
        self.call("send_private_msg", json!({ "user_id": user_id, "message": message })).await
    }

    /// ⚠️ **撤回**自己发出的消息。会让消息在对方那边也消失，
    /// 与「本地手动删除」（只写墓碑表）完全不是一件事。
    pub async fn delete_msg(&self, message_id: &str) -> Result<Value> {
        self.call("delete_msg", json!({ "message_id": message_id })).await
    }

    /* --- 标记已读（同步到 QQ） --- */

    pub fn mark_private_msg_as_read(&self, user_id: i64) {
        self.fire("mark_private_msg_as_read", json!({ "user_id": user_id }));
    }

    pub fn mark_group_msg_as_read(&self, group_id: i64) {
        self.fire("mark_group_msg_as_read", json!({ "group_id": group_id }));
    }

    pub fn mark_all_as_read(&self) {
        self.fire("_mark_all_as_read", json!({}));
    }

    /* --- 媒体 --- */

    /// 消息段 `url` 缺失或过期时的兜底：用 `file` 换一个直链。
    pub async fn get_image(&self, file: &str) -> Result<Value> {
        self.call_heavy("get_image", json!({ "file": file })).await
    }

    pub async fn download_file(&self, url: &str, thread_count: i64) -> Result<Value> {
        self.call_heavy(
            "download_file",
            json!({ "url": url, "thread_count": thread_count }),
        )
        .await
    }
}

#[cfg(test)]
#[allow(uncommon_codepoints)]
mod tests {
    use super::*;

    #[test]
    fn 帧形状与协议一致() {
        let f = build_frame("send_group_msg", json!({ "group_id": 1 }), "7");
        assert_eq!(f["action"], "send_group_msg");
        assert_eq!(f["params"]["group_id"], 1);
        assert_eq!(f["echo"], "7");
    }

    #[test]
    fn 响应_成功() {
        let r = extract_response(&json!({
            "status": "ok", "retcode": 0, "echo": "7",
            "data": { "message_id": 12345 }
        }));
        assert!(r.ok);
        assert_eq!(r.echo.as_deref(), Some("7"));
        assert_eq!(sent_message_id(&r).as_deref(), Some("12345"));
    }

    #[test]
    fn 响应_失败带原因() {
        let r = extract_response(&json!({
            "status": "failed", "retcode": 100, "echo": "8", "message": "群不存在"
        }));
        assert!(!r.ok);
        assert_eq!(r.error.as_deref(), Some("群不存在"));
    }

    #[test]
    fn 响应_没有_message_时用_retcode_兜底() {
        let r = extract_response(&json!({ "retcode": 1404, "echo": "9" }));
        assert!(!r.ok);
        assert!(r.error.unwrap().contains("1404"));
    }

    #[test]
    fn 响应_数字_echo_也认() {
        let r = extract_response(&json!({ "retcode": 0, "echo": 42 }));
        assert_eq!(r.echo.as_deref(), Some("42"));
    }

    #[test]
    fn 响应_事件推送不被误判() {
        // 事件推送没有 echo
        assert!(!is_response(&json!({ "post_type": "message", "message_id": 1 })));
        assert!(is_response(&json!({ "retcode": 0, "echo": "1" })));
    }

    #[test]
    fn 数组字段_三种包裹形态() {
        assert_eq!(items_of(&json!([1, 2]), &[]).len(), 2);
        assert_eq!(items_of(&json!({ "messages": [1, 2, 3] }), &["messages"]).len(), 3);
        assert_eq!(items_of(&json!({ "list": [1] }), &["messages", "list"]).len(), 1);
        assert!(items_of(&json!({ "other": 1 }), &["messages"]).is_empty());
        assert!(items_of(&Value::Null, &["messages"]).is_empty());
    }

    #[tokio::test]
    async fn 调用_发出帧并等到响应() {
        let (bus, mut rx) = ActionBus::channel();

        let handle = tokio::spawn({
            let bus = bus.clone();
            async move { bus.call("get_login_info", json!({})).await }
        });

        let frame = rx.recv().await.expect("应该收到一帧");
        assert_eq!(frame["action"], "get_login_info");
        let echo = frame["echo"].as_str().unwrap().to_string();

        bus.resolve(ActionResponse {
            echo: Some(echo),
            ok: true,
            error: None,
            data: json!({ "user_id": 10001, "nickname": "我" }),
        });

        let data = handle.await.unwrap().unwrap();
        assert_eq!(data["user_id"], 10001);
    }

    #[tokio::test]
    async fn 调用_失败响应变成错误() {
        let (bus, mut rx) = ActionBus::channel();
        let handle = tokio::spawn({
            let bus = bus.clone();
            async move { bus.call("delete_msg", json!({ "message_id": "1" })).await }
        });

        let frame = rx.recv().await.unwrap();
        let echo = frame["echo"].as_str().unwrap().to_string();
        bus.resolve(ActionResponse {
            echo: Some(echo),
            ok: false,
            error: Some("消息不存在".into()),
            data: Value::Null,
        });

        let err = handle.await.unwrap().unwrap_err().to_string();
        assert!(err.contains("消息不存在"), "{err}");
    }

    #[tokio::test]
    async fn 调用_超时不会永远挂着() {
        let (bus, _rx) = ActionBus::channel();
        let err = bus
            .call_with_timeout("get_status", json!({}), 30)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("超时"), "{err}");
    }

    #[tokio::test]
    async fn 断线_作废所有在途请求() {
        let (bus, _rx) = ActionBus::channel();
        let handle = tokio::spawn({
            let bus = bus.clone();
            async move { bus.call("get_status", json!({})).await }
        });
        // 让 call 先登记进去
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        bus.fail_all("连接已断开");

        let err = handle.await.unwrap().unwrap_err().to_string();
        assert!(err.contains("连接已断开"), "{err}");
    }

    #[tokio::test]
    async fn 无人认领的响应不报错() {
        let (bus, _rx) = ActionBus::channel();
        assert!(!bus.resolve(ActionResponse {
            echo: Some("999".into()),
            ok: true,
            error: None,
            data: Value::Null,
        }));
    }

    #[tokio::test]
    async fn 没有_echo_的响应被忽略() {
        let (bus, _rx) = ActionBus::channel();
        assert!(!bus.resolve(ActionResponse::empty()));
    }
}
