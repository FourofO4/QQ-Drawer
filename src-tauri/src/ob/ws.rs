//! WebSocket 连接、心跳、指数退避重连与「重连补齐」（§4.2 / §4.8 #7）。
//!
//! 一个会话的生命周期：
//!
//! ```text
//!   session()  ─ 建连 → 装 ActionBus → 取 self_id → 补齐 → 读循环 ─ 断开
//!                                                            │
//!        ◀──────────────── run() 退避重试 ───────────────────┘
//! ```
//!
//! 三条约束：
//!  1. **断线不丢状态**：本地库是唯一长期副本，重连只补「会话列表 + 当前会话最后一页」，
//!     不做全量补偿（NapCat 也给不出来，§4.8 #7）。
//!  2. **一次只有一条连接**：`session` 返回时一定会把 `ActionBus` 从 `AppState` 摘掉并
//!     作废在途请求，避免旧连接的响应去唤醒新连接的等待者（`echo` 只在单条连接内唯一）。
//!  3. **退避是纯函数**：`backoff_ms` 可以单测，不用起 socket。

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::appstate::{emit, emit_conversations, events, AppState};
use crate::model::{ConnInfo, Peer};
use crate::ob::api::{self, ActionBus};
use crate::ob::{event, segments};
use crate::store::conversation as conv_store;

/// 首连与重连补齐时拉的会话条数（FR-14）
pub const RECENT_COUNT: i64 = 20;
/// 一次历史分页的条数（与前端 `loadMessages(..., 30)` 对齐）
pub const HISTORY_PAGE: i64 = 30;
/// 静默多久算连接已死。
///
/// NapCat 默认每 5 秒发一个 `meta_event` 心跳，90 秒没有任何报文意味着
/// TCP 还连着但对端已经不工作了——这时候**必须**主动断开重连，
/// 否则会出现「界面显示已连接，消息再也不来」。
pub const IDLE_TIMEOUT_SECS: u64 = 90;

/* ------------------------------ 纯函数 ------------------------------ */

const BACKOFF_BASE_MS: u64 = 500;
const BACKOFF_MAX_MS: u64 = 30_000;

/// 指数退避：500 / 1000 / 2000 / … 封顶 30 秒。
///
/// 不引入随机抖动：NapCat 就在本机，不存在"一批客户端同时重连打垮服务端"的场景，
/// 确定性反而让行为可预测、可测试。
pub fn backoff_ms(attempt: u32) -> u64 {
    // 1 << 6 = 64，乘 500 已经超过上限，所以指数到 6 就够
    let shift = attempt.min(6);
    BACKOFF_BASE_MS.saturating_mul(1u64 << shift).min(BACKOFF_MAX_MS)
}

/// 是否还要继续重连。用户关掉「自动重连」后只做首次尝试。
pub fn should_retry(auto_reconnect: bool, attempt: u32) -> bool {
    auto_reconnect || attempt == 0
}

/// 把 `access_token` 拼进 URL（NFR-11：连接必须带 token）。
///
/// NapCat 两种都认：`?access_token=` 查询串，以及 `Authorization: Bearer` 头。
/// 两个都带上——某些反向代理会吃掉其中一个。
pub fn with_token(url: &str, token: &str) -> String {
    if token.is_empty() || url.contains("access_token=") {
        return url.to_string();
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}access_token={token}")
}

/* ------------------------------ 连接生命周期 ------------------------------ */

/// 常驻任务：连上 → 断了 → 退避 → 再连。只有 `AppHandle` 掉线才会结束。
pub async fn run(app: AppHandle, state: Arc<AppState>) {
    let mut attempt: u32 = 0;

    loop {
        if let Err(e) = session(&app, &state).await {
            tracing::warn!(error = %e, attempt, "连接中断");
        }

        // 收尾：作废在途请求并摘掉总线。
        // 顺序很重要——先 fail_all 再摘，否则 `call()` 会因为总线没了直接报"未连接"。
        let bus = state.bus.write().take();
        if let Some(bus) = bus {
            bus.fail_all("连接已断开");
        }

        let settings = state.settings_snapshot();
        let self_id = state.self_id();
        state.set_conn(ConnInfo::disconnected(if self_id > 0 { Some(self_id) } else { None }));
        emit(&app, events::CONN_STATE, state.conn_info());

        if !should_retry(settings.auto_reconnect, attempt) {
            tracing::info!("自动重连已关闭，停止重试");
            return;
        }

        let wait = backoff_ms(attempt);
        attempt = attempt.saturating_add(1);
        tracing::info!(wait_ms = wait, "{} 毫秒后重连", wait);

        tokio::select! {
            // 「点击重试」：立刻重连，并把退避重置回最短
            _ = state.wake.notified() => {
                attempt = 0;
                tracing::info!("收到重试请求，立即重连");
            }
            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
        }
    }
}

/// 一条连接从建立到断开。返回 `Err` 表示断开原因（正常关闭也算）。
async fn session(app: &AppHandle, state: &Arc<AppState>) -> Result<()> {
    let settings = state.settings_snapshot();
    state.set_conn(ConnInfo::connecting());
    emit(app, events::CONN_STATE, state.conn_info());

    let url = with_token(&settings.ws_url, &settings.access_token);
    let mut req = url
        .as_str()
        .into_client_request()
        .with_context(|| format!("WebSocket 地址非法: {url}"))?;
    if !settings.access_token.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", settings.access_token)) {
            req.headers_mut().insert(AUTHORIZATION, v);
        }
    }

    let (stream, _resp) = tokio_tungstenite::connect_async(req)
        .await
        .with_context(|| format!("连接失败: {url}"))?;
    tracing::info!(url, "WebSocket 已连接");

    let (mut write, mut read) = stream.split();

    // 出站帧通道。ActionBus 只管把 Value 丢进来，真正的写由下面的任务做。
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let bus = ActionBus::new(tx);
    *state.bus.write() = Some(bus.clone());

    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let text = frame.to_string();
            if let Err(e) = write.send(Message::Text(text.into())).await {
                tracing::debug!(error = %e, "写 WebSocket 失败");
                break;
            }
        }
    });

    // 1) 取 self_id —— @我判定、"自己发的消息"判定都靠它，所以是连接后的第一件事
    match bus.get_login_info().await {
        Ok(data) => {
            let self_id = crate::ob::model::as_i64(&data, "user_id").unwrap_or(0);
            state.self_id.store(self_id, Ordering::Relaxed);
            let nick = crate::ob::model::as_str(&data, "nickname").unwrap_or_default();
            tracing::info!(self_id, nickname = %nick, "登录信息已获取");
        }
        Err(e) => {
            // 拿不到 self_id 不算致命：库里的数据还能看，只是 @我 判定会退化
            tracing::warn!(error = %e, "获取登录信息失败");
        }
    }

    // 2) 记 NapCat 版本，排查兼容问题时省一轮问答（NFR-12：日志不含消息正文）
    if let Ok(v) = bus.get_version_info().await {
        let ver = crate::ob::model::as_str(&v, "version").unwrap_or_default();
        let app_name = crate::ob::model::as_str(&v, "app_name").unwrap_or_default();
        tracing::info!(app_name = %app_name, version = %ver, "上游版本");
    }

    state.set_conn(ConnInfo::connected(state.self_id()));
    emit(app, events::CONN_STATE, state.conn_info());

    // 3) 重连补齐（§4.8 #7）
    resync(app, state, &bus).await;

    // 4) 读循环
    let reason = read_loop(app, state, &bus, &mut read).await;

    // 5) 收尾：停掉写任务，让出站通道立刻关闭（bus 之后会被 run() 摘掉并作废）
    writer.abort();
    reason
}

/// 读循环。返回 `Err` 表示断开原因。
async fn read_loop(
    app: &AppHandle,
    state: &Arc<AppState>,
    bus: &ActionBus,
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> Result<()> {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(IDLE_TIMEOUT_SECS), read.next()).await;

        let msg = match next {
            Err(_) => anyhow::bail!("{IDLE_TIMEOUT_SECS} 秒没有收到任何报文，判定连接已死"),
            Ok(None) => anyhow::bail!("对端关闭了连接"),
            Ok(Some(Err(e))) => return Err(anyhow::Error::new(e).context("读取 WebSocket 失败")),
            Ok(Some(Ok(m))) => m,
        };

        // Ping/Pong 与二进制帧不进业务：tungstenite 会自己回 Pong（在 WebSocketStream 的
        // poll_next 里顺带 flush），所以这里只需要忽略
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => match String::from_utf8(b.to_vec()) {
                Ok(s) => s,
                Err(_) => continue,
            },
            Message::Close(_) => anyhow::bail!("对端发来关闭帧"),
            _ => continue,
        };

        let parsed: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                // 一条坏报文不该让整条连接倒下
                tracing::warn!(error = %e, "报文不是合法 JSON，已忽略");
                continue;
            }
        };

        // action 响应 → 唤醒对应的等待者；事件 → 走解析管线。
        // 判定顺序不能反：某些版本的响应里也带 post_type。
        if api::is_response(&parsed) {
            bus.resolve(api::extract_response(&parsed));
        } else {
            event::handle(app, state, &parsed).await;
        }
    }
}

/* ------------------------------ 重连补齐 ------------------------------ */

/// 重连成功后：更新会话列表，再把「当前打开的那个会话」的最后一页补回来。
///
/// 刻意**不做全量补偿**：NapCat 侧只留约 5000 条，逐会话回补会把刚建立的连接打满，
/// 而且用户真正在意的是"我正在看的这个会话别缺消息"。
pub async fn resync(app: &AppHandle, state: &Arc<AppState>, bus: &ActionBus) {
    // 1) 会话种子（FR-14）
    match bus.get_recent_contact(RECENT_COUNT).await {
        Ok(data) => {
            let items = api::items_of(&data, &["list"]);
            let seed = segments::recent_contact_seed(&items);
            if !seed.is_empty() {
                let n = seed.len();
                if let Err(e) = state.db.tx(|c| conv_store::seed_many(c, &seed)) {
                    tracing::warn!(error = %e, "会话种子写入失败");
                } else {
                    tracing::info!(count = n, "会话种子已更新");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "拉取会话列表失败"),
    }
    emit_conversations(app, state);

    // 2) 当前打开的会话补最后一页
    let viewing = *state.viewing.read();
    let expanded = state.expanded.load(Ordering::Relaxed);
    if let (Some(peer), true) = (viewing, expanded) {
        match remote_history(bus, peer, 0, HISTORY_PAGE).await {
            Ok(items) if !items.is_empty() => {
                let added = event::ingest_history(app, state, state.self_id(), &items);
                if added > 0 {
                    tracing::info!(peer = %peer.key(), added, "补齐当前会话");
                }
            }
            Ok(_) => {}
            Err(e) => tracing::debug!(error = %e, "补齐当前会话失败"),
        }
    }
}

/// 拉远端历史（§4.6）。
///
/// `message_seq = 0` 是 OneBot 约定的「最新一页」。
pub async fn remote_history(
    bus: &ActionBus,
    peer: Peer,
    message_seq: i64,
    count: i64,
) -> Result<Vec<Value>> {
    let data = if peer.is_group() {
        bus.get_group_msg_history(peer.peer_id, message_seq, count).await?
    } else {
        bus.get_friend_msg_history(peer.peer_id, message_seq, count).await?
    };
    Ok(api::items_of(&data, &["messages", "message"]))
}

/// 由分页游标推 OneBot 的 `message_seq`。
///
/// 本地 `seq` 是自增序号，与上游的 `message_seq` 不是一回事。这里能把上游 id 当数字
/// 解析出来时就用它当游标，否则退回 0（＝最新一页）——退化的后果只是多补一页重复数据，
/// 写入是幂等的，不会产生重复消息。
pub fn history_cursor(fallback: Option<i64>) -> i64 {
    fallback.filter(|v| *v > 0).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 退避序列与封顶() {
        assert_eq!(backoff_ms(0), 500);
        assert_eq!(backoff_ms(1), 1_000);
        assert_eq!(backoff_ms(2), 2_000);
        assert_eq!(backoff_ms(3), 4_000);
        assert_eq!(backoff_ms(4), 8_000);
        assert_eq!(backoff_ms(5), 16_000);
        assert_eq!(backoff_ms(6), 30_000, "封顶 30 秒");
        assert_eq!(backoff_ms(50), 30_000, "极大值也不能溢出");
    }

    #[test]
    fn 关掉自动重连后只试一次() {
        assert!(should_retry(true, 0));
        assert!(should_retry(true, 9));
        assert!(should_retry(false, 0), "首次尝试总是允许");
        assert!(!should_retry(false, 1), "失败一次之后就不再重试");
    }

    #[test]
    fn token_拼进查询串() {
        assert_eq!(
            with_token("ws://127.0.0.1:3001", "abc"),
            "ws://127.0.0.1:3001?access_token=abc"
        );
        assert_eq!(
            with_token("ws://127.0.0.1:3001/ws?a=1", "abc"),
            "ws://127.0.0.1:3001/ws?a=1&access_token=abc"
        );
    }

    #[test]
    fn token_为空或已存在时不重复拼() {
        assert_eq!(with_token("ws://x:1", ""), "ws://x:1");
        assert_eq!(
            with_token("ws://x:1?access_token=old", "new"),
            "ws://x:1?access_token=old",
            "地址里已经有 token 就不要画蛇添足"
        );
    }

    #[test]
    fn 分页游标回退到最新一页() {
        assert_eq!(history_cursor(Some(456)), 456);
        assert_eq!(history_cursor(Some(0)), 0);
        assert_eq!(history_cursor(Some(-1)), 0);
        assert_eq!(history_cursor(None), 0);
    }
}
