//! 事件解析 → 规范模型（§4.2 / §4.8）。
//!
//! 这一层是「协议墙」的接收侧：上游的 `post_type` / `notice_type` / 字段名到这里为止，
//! 往下（`store` / `cmd` / 前端）只认 `crate::model`。
//!
//! 拆成两半，是为了让最容易出错的部分可测：
//!  · **解析**（`parse_message` / `parse_recall`）——**纯函数**，输入一个 `Value` 输出规范结构；
//!  · **落地**（`handle`）——碰数据库、发事件、派发图片下载。
//!
//! 幂等的责任在 `store::message::upsert`（`message_id` 唯一），不在这一层。
//! 所以"同一条消息被事件与历史拉取各送一次"完全不用在这里小心避让。

use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use tauri::AppHandle;

use crate::appstate::{emit, events, AppState};
use crate::model::{
    send_state, ConversationDto, MessageAddedPayload, Peer, Seg,
};
use crate::ob::segments::{self, Parsed};
use crate::sched::notify::{self, NotifyInput};
use crate::store::{conversation as conv_store, member as member_store, message as message_store, now_ms};

/* ------------------------------ 解析结果 ------------------------------ */

/// 一条待落地的消息事件。
#[derive(Clone, Debug, PartialEq)]
pub struct MessageEvent {
    pub message_id: String,
    pub peer: Peer,
    /// 群临时会话：peer 仍然是私聊，但会话名要标注「群临时」（§4.5）
    pub temp_from_group: Option<i64>,
    pub message_seq: Option<i64>,
    pub ts: i64,
    pub sender_id: i64,
    pub sender_card: Option<String>,
    pub sender_nickname: Option<String>,
    pub is_self: bool,
    pub parsed: Parsed,
}

impl MessageEvent {
    /// 发送者展示名：群名片 > 昵称（FR-14）
    pub fn sender_name(&self) -> Option<String> {
        let n = conv_store::best_name(None, self.sender_card.as_deref(), self.sender_nickname.as_deref());
        if n.is_empty() {
            None
        } else {
            Some(n)
        }
    }
}

/// 撤回事件。
#[derive(Clone, Debug, PartialEq)]
pub struct RecallEvent {
    pub peer: Peer,
    pub message_id: String,
    /// 执行撤回的人；`None` 表示上游没给
    pub operator_id: Option<i64>,
}

/* ------------------------------ 纯函数：解析 ------------------------------ */

/// 是不是一个事件推送（`echo` 只出现在 action 响应上）。
pub fn is_event(v: &Value) -> bool {
    !crate::ob::api::is_response(v) && v.get("post_type").is_some()
}

/// 上游 `message` 字段可能是消息段数组，也可能是 CQ 码字符串。
///
/// 字符串形态（`"[CQ:at,qq=1] 在吗"`）在 NapCat 上默认不会出现，但
/// `message_format: "string"` 的配置下会——不解析会直接丢内容，所以退化成一条文本段，
/// 至少不丢字。
fn raw_segments(v: &Value) -> Vec<Value> {
    match v.get("message") {
        Some(Value::Array(arr)) => arr.clone(),
        Some(Value::String(s)) if !s.is_empty() => {
            vec![serde_json::json!({ "type": "text", "data": { "text": s } })]
        }
        _ => Vec::new(),
    }
}

/// 解析 message / message_sent 事件。
pub fn parse_message(v: &Value, self_id: i64) -> Option<MessageEvent> {
    let post_type = v.get("post_type").and_then(|x| x.as_str()).unwrap_or("");
    if post_type != "message" && post_type != "message_sent" {
        return None;
    }

    let group_id = crate::ob::model::as_i64(v, "group_id").filter(|x| *x > 0);
    let user_id = crate::ob::model::as_i64(v, "user_id").filter(|x| *x > 0)?;
    let msg_type = crate::ob::model::as_str(v, "message_type").unwrap_or_default();
    let sub_type = crate::ob::model::as_str(v, "sub_type").unwrap_or_default();

    // 群消息判定：优先看声明，其次看有没有 group_id
    let is_group = msg_type == "group" || (msg_type.is_empty() && group_id.is_some() && sub_type != "group");

    let (peer, temp_from_group) = if is_group {
        let gid = group_id?;
        (Peer::group(gid), None)
    } else if sub_type == "group" {
        // 群临时会话：挂 group_id 但 message_type 是 private
        (Peer::private(user_id), group_id)
    } else {
        (Peer::private(user_id), None)
    };

    let message_id = match v.get("message_id") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return None,
    };
    if message_id.is_empty() {
        return None;
    }

    let sender = v.get("sender").cloned().unwrap_or(Value::Null);
    let sender_id = crate::ob::model::as_i64(&sender, "user_id").unwrap_or(user_id);
    let ts = crate::ob::model::as_i64(v, "time")
        .map(crate::ob::model::normalize_ts)
        .unwrap_or_else(now_ms);

    let parsed = segments::parse(&raw_segments(v), self_id);

    Some(MessageEvent {
        message_id,
        peer,
        temp_from_group,
        message_seq: crate::ob::model::as_i64(v, "message_seq"),
        ts,
        sender_id,
        sender_card: crate::ob::model::as_str(&sender, "card").filter(|s| !s.trim().is_empty()),
        sender_nickname: crate::ob::model::as_str(&sender, "nickname")
            .filter(|s| !s.trim().is_empty()),
        is_self: sender_id == self_id && self_id != 0,
        parsed,
    })
}

/// 解析撤回通知（`notice_type = group_recall | friend_recall`）。
pub fn parse_recall(v: &Value) -> Option<RecallEvent> {
    if v.get("post_type").and_then(|x| x.as_str()) != Some("notice") {
        return None;
    }
    let notice_type = crate::ob::model::as_str(v, "notice_type").unwrap_or_default();
    let message_id = crate::ob::model::as_str(v, "message_id")
        .or_else(|| crate::ob::model::as_i64(v, "message_id").map(|n| n.to_string()))?;
    if message_id.is_empty() {
        return None;
    }

    let peer = match notice_type.as_str() {
        "group_recall" => Peer::group(crate::ob::model::as_i64(v, "group_id")?),
        "friend_recall" => Peer::private(crate::ob::model::as_i64(v, "user_id")?),
        _ => return None,
    };

    Some(RecallEvent {
        peer,
        message_id,
        operator_id: crate::ob::model::as_i64(v, "operator_id"),
    })
}

/* ------------------------------ 落地 ------------------------------ */

/// 事件总入口。ws 读半边每收到一个事件报文调用一次。
pub async fn handle(app: &AppHandle, state: &Arc<AppState>, v: &Value) {
    match v.get("post_type").and_then(|x| x.as_str()) {
        Some("message") | Some("message_sent") => handle_message(app, state, v),
        Some("notice") => handle_notice(app, state, v),
        // meta_event（生命周期/心跳）与 request（加好友/加群）在 MVP 里不处理：
        // 心跳只用来判定连接活性，那个逻辑在 ws 层按"收到任何报文"算
        _ => {}
    }
}

fn handle_message(app: &AppHandle, state: &Arc<AppState>, v: &Value) {
    let self_id = state.self_id();
    let Some(ev) = parse_message(v, self_id) else {
        tracing::debug!("无法解析的消息事件，已忽略");
        return;
    };

    let peer = ev.peer;
    let sender_name = ev.sender_name();

    // 1) 会话先落位。
    //
    // 名字传空串有两个原因：群名不在消息事件里（要另拉 `get_group_info`）；
    // 群临时会话的名字需要拼成「群名（群临时）」而不是直接用昵称。
    // `ensure` 的 ON CONFLICT 分支不会用空串覆盖已有名字，所以这里交给下面的兜底逻辑补。
    let name_hint = if peer.is_group() || ev.temp_from_group.is_some() {
        String::new()
    } else {
        sender_name.clone().unwrap_or_default()
    };
    if let Err(e) = state.db.tx(|c| {
        conv_store::ensure(c, peer, &name_hint, None)?;
        if let Some(gid) = ev.temp_from_group {
            let group_name = conv_store::get(c, Peer::group(gid))?
                .map(|x| x.name)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("群{gid}"));
            if let Some(cur) = conv_store::get(c, peer)? {
                if cur.name.is_empty() {
                    let temp = conv_store::temp_group_name(&group_name, &name_hint);
                    conv_store::set_name(c, peer, &temp)?;
                }
            }
        }
        Ok(())
    }) {
        tracing::warn!(error = %e, "会话落位失败");
    }
    ensure_conversation_name(app, state, peer);

    // 2) 补齐 @ 的昵称（消息段里没带名字的那些）
    let mut parsed = ev.parsed.clone();
    fill_at_names(state, peer, &mut parsed);

    // 3) 入库。`inc_unread` 只在「没在看着这个会话」且「不是自己发的」时为真（FR-31）
    let viewing = state.is_viewing(peer);
    let inc_unread = !viewing && !ev.is_self;
    let preview = parsed.plain_text();
    let new_msg = message_store::NewMessage {
        message_id: ev.message_id.clone(),
        peer,
        ts: ev.ts,
        sender_id: ev.sender_id,
        sender_name: sender_name.clone(),
        is_self: ev.is_self,
        segments: parsed.segments.clone(),
        reply_to: reply_to_of(&parsed),
        is_at_me: parsed.is_at_me,
        send_state: send_state::CONFIRMED,
    };

    let inserted = match state.db.tx(|c| {
        let fresh = message_store::upsert(c, &new_msg)?;
        conv_store::touch_last(c, peer, ev.ts, &preview, sender_name.as_deref(), inc_unread, parsed.is_at_me)?;
        Ok(fresh)
    }) {
        Ok(fresh) => fresh,
        Err(e) => {
            tracing::warn!(error = %e, "消息入库失败");
            return;
        }
    };

    // 4) 派发图片下载（收到即入队，§4.8 #6）
    if parsed.has_image() {
        dispatch_images(app, state, &ev.message_id, &parsed);
    }

    // 5) 把消息与该会话的最新快照一起推给前端（NFR-04 ≤ 150 ms）。
    //
    // 新消息走 `msg_added`（带会话快照，前端顺势更新未读与预览，不用回拉整个列表）；
    // 已存在的消息被改写（图片路径回填、正文更新）走 `msg_updated`（裸 DTO）。
    let snapshot = state.db.with(|c| conv_store::get(c, peer)).ok().flatten();
    match state.db.with(|c| message_store::get(c, &ev.message_id)) {
        Ok(Some(dto)) => {
            if inserted {
                emit(
                    app,
                    events::MSG_ADDED,
                    MessageAddedPayload { message: dto, conversation: snapshot.clone() },
                );
            } else {
                emit(app, events::MSG_UPDATED, dto);
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "读取刚落库的消息失败"),
    }

    // 自己发的消息不提醒（不然会出现"我发完自己闪自己"）
    if ev.is_self {
        return;
    }
    emit_notify(app, state, peer, parsed.is_at_me, snapshot.as_ref());
}

/// 撤回：标记但保留原文（FR-44），然后推一条更新给前端。
fn handle_notice(app: &AppHandle, state: &Arc<AppState>, v: &Value) {
    let Some(ev) = parse_recall(v) else {
        return;
    };
    let changed = match state
        .db
        .tx(|c| message_store::mark_recalled(c, &ev.message_id, ev.operator_id))
    {
        Ok(changed) => changed,
        Err(e) => {
            tracing::warn!(error = %e, "标记撤回失败");
            return;
        }
    };
    if !changed {
        // 本地没有这条（可能已被墓碑删掉，或者不在我们同步过的范围里）
        tracing::debug!(message_id = %ev.message_id, "撤回了一条本地没有的消息");
        return;
    }
    match state.db.with(|c| message_store::get(c, &ev.message_id)) {
        Ok(Some(dto)) => emit(app, events::MSG_UPDATED, dto),
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "读取撤回后的消息失败"),
    }
}

/// 计算出提醒意图并发给前端。前端只把 `NotifyIntent` 翻译成 CSS 类，不做判定（§3.5）。
fn emit_notify(
    app: &AppHandle,
    state: &Arc<AppState>,
    peer: Peer,
    at_me: bool,
    snapshot: Option<&ConversationDto>,
) {
    let muted = state
        .db
        .with(|c| conv_store::is_muted(c, peer))
        .unwrap_or(false);
    let viewing = state.is_viewing(peer);
    let in_tabs = snapshot.map(|c| c.tab_order.is_some()).unwrap_or(false);
    let settings = state.settings_snapshot();

    let input = NotifyInput::new(viewing, muted)
        .with_at_me(at_me)
        .with_place(in_tabs, viewing)
        .with_muted_mention_flash(settings.flash_on_mention_when_muted);

    let mut intent = notify::decide(&input);
    intent.peer_type = peer.peer_type;
    intent.peer_id = peer.peer_id;

    // 折叠条的**内容**由全局优先级决定（FR-33），不由这条消息决定。
    // `decide` 只能表态到"这条消息想不想上折叠条"，至于最后显示谁，
    // 必须拿更新后的整表重新算一次——否则静音会话会抢到位置。
    let list = state.db.with(conv_store::list).unwrap_or_default();
    intent.claim_bar = notify::pick_bar(&list)
        .map(|w| w.peer_type == peer.peer_type && w.peer_id == peer.peer_id)
        .unwrap_or(false);

    emit(app, events::NOTIFY, intent);
}

/// 从 `reply` 段里取被引用的 message_id。
fn reply_to_of(parsed: &Parsed) -> Option<String> {
    parsed.segments.iter().find_map(|s| match s {
        Seg::Reply { id } => Some(id.clone()),
        _ => None,
    })
}

/// 把 `at` 段里缺的昵称从群成员缓存里补上（§4.8 #4）。
fn fill_at_names(state: &Arc<AppState>, peer: Peer, parsed: &mut Parsed) {
    if !peer.is_group() || parsed.unknown_at_names().is_empty() {
        return;
    }
    for qq in parsed.unknown_at_names() {
        if let Ok(Some(name)) = state.db.with(|c| member_store::name_of(c, peer.peer_id, qq)) {
            parsed.fill_at_name(qq, &name);
        }
    }
}

/// 图片下载派发。
///
/// 上游直链有时会缺 `url`（只有 `file`），那条路径走 `get_image` 兜底换直链；
/// 这里先只处理有直链的，缺链的已经在上游被标成 `FAILED`（`ob::segments::image_from`）。
fn dispatch_images(app: &AppHandle, state: &Arc<AppState>, message_id: &str, parsed: &Parsed) {
    let tasks = parsed.images.clone();
    if tasks.is_empty() {
        return;
    }
    let app = app.clone();
    let state = state.clone();
    let message_id = message_id.to_string();
    let images_total = tasks.len();

    // 图片下载是"发完就不管"的活，但必须能在应用退出时自然结束 —— 用 tauri 的
    // async runtime 派生，而不是 std::thread
    tauri::async_runtime::spawn(async move {
        let client = match crate::media::http_client() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "创建图片下载客户端失败");
                return;
            }
        };
        for task in tasks {
            crate::media::fetch_and_store(&app, &state, &client, &message_id, &task).await;
        }
        tracing::debug!(message_id, images_total, "图片下载批次结束");
    });
}

/// 会话名兜底：群里第一次见到某个群、或者私聊第一次见到某人时，事件里没有名字。
///
/// 只在该会话当前名字为空时才发请求——已经有名字就绝不打扰上游（几十个会话逐个拉
/// `get_group_info` 是纯粹的浪费）。
fn ensure_conversation_name(app: &AppHandle, state: &Arc<AppState>, peer: Peer) {
    let need = match state.db.with(|c| conv_store::get(c, peer)) {
        Ok(Some(c)) => c.name.trim().is_empty(),
        _ => true,
    };
    if !need {
        return;
    }

    let Some(bus) = state.bus.read().clone() else {
        return;
    };
    let app = app.clone();
    let state = state.clone();

    tauri::async_runtime::spawn(async move {
        let name = if peer.is_group() {
            match bus.get_group_info(peer.peer_id).await {
                Ok(data) => crate::ob::model::as_str(&data, "group_name").unwrap_or_default(),
                Err(e) => {
                    tracing::debug!(error = %e, "拉群信息失败，会话名暂缺");
                    String::new()
                }
            }
        } else {
            // 私聊：从好友列表里找备注（FR-14 的备注优先级最高）
            match bus.get_friend_list().await {
                Ok(data) => find_friend_name(&data, peer.peer_id).unwrap_or_default(),
                Err(e) => {
                    tracing::debug!(error = %e, "拉好友列表失败，会话名暂缺");
                    String::new()
                }
            }
        };

        if name.trim().is_empty() {
            return;
        }
        if let Err(e) = state.db.tx(|c| conv_store::set_name(c, peer, &name)) {
            tracing::warn!(error = %e, "写入会话名失败");
            return;
        }
        crate::appstate::emit_conversations(&app, &state);
    });
}

/// 从 `get_friend_list` 的返回里挑某个好友的展示名：备注 > 昵称。
pub fn find_friend_name(data: &Value, user_id: i64) -> Option<String> {
    let items = crate::ob::api::items_of(data, &["list", "friends"]);
    for it in items {
        if crate::ob::model::as_i64(&it, "user_id") == Some(user_id) {
            let remark = crate::ob::model::as_str(&it, "remark").unwrap_or_default();
            let nickname = crate::ob::model::as_str(&it, "nickname").unwrap_or_default();
            let name = conv_store::best_name(Some(&remark), None, Some(&nickname));
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// 把 `get_group_member_list` 的返回转成 `member_cache` 的入参。
pub fn member_rows(data: &Value) -> Vec<(i64, Option<String>, Option<String>, Option<String>)> {
    crate::ob::api::items_of(data, &["list"])
        .into_iter()
        .filter_map(|it| {
            let user_id = crate::ob::model::as_i64(&it, "user_id")?;
            Some((
                user_id,
                crate::ob::model::as_str(&it, "nickname"),
                crate::ob::model::as_str(&it, "card"),
                crate::ob::model::as_str(&it, "role"),
            ))
        })
        .collect()
}

/* ------------------------------ 历史拉取 ------------------------------ */

/// 解析一条历史记录。
///
/// 历史接口返回的对象与事件推送**基本同形但不完全同形**：有的版本会带上 `post_type`，
/// 有的只给消息本体。这里缺什么补什么，然后再交给同一个 `parse_message`——
/// 两条路径共用一套解析逻辑，避免"实时消息能渲染、翻历史就变成占位符"这类偏差。
pub fn parse_history_item(v: &Value, self_id: i64) -> Option<MessageEvent> {
    if v.get("post_type").is_some() {
        return parse_message(v, self_id);
    }
    let mut owned = v.clone();
    if let Some(obj) = owned.as_object_mut() {
        obj.insert("post_type".into(), Value::String("message".into()));
    }
    parse_message(&owned, self_id)
}

/// 把远端历史批量落地。返回**新插入**的条数。
///
/// 为什么和 `handle_message` 分开：历史不是新消息。
/// 它不该加未读、不该闪折叠条、不该留痕——把这两条路径混在一起，
/// 翻一次历史就会把未读数刷成几十，而用户什么都没收到。
///
/// 幂等仍然由 `store::message::upsert` 保证，所以「同一页被补两次」是安全的。
pub fn ingest_history(app: &AppHandle, state: &Arc<AppState>, self_id: i64, items: &[Value]) -> usize {
    let mut inserted = 0usize;

    for item in items {
        let Some(ev) = parse_history_item(item, self_id) else {
            continue;
        };

        // 墓碑命中：用户明确删掉的消息，远端补齐也**不能**让它回来（FR-45）
        match state.db.with(|c| crate::store::tombstone::is_deleted(c, &ev.message_id)) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(error = %e, "查询墓碑失败");
                continue;
            }
        }

        let peer = ev.peer;
        if let Err(e) = state.db.tx(|c| conv_store::ensure(c, peer, "", None)) {
            tracing::warn!(error = %e, "历史消息的会话落位失败");
            continue;
        }
        ensure_conversation_name(app, state, peer);

        let mut parsed = ev.parsed.clone();
        fill_at_names(state, peer, &mut parsed);
        let name = ev.sender_name();

        let new_msg = message_store::NewMessage {
            message_id: ev.message_id.clone(),
            peer,
            ts: ev.ts,
            sender_id: ev.sender_id,
            sender_name: name,
            is_self: ev.is_self,
            segments: parsed.segments.clone(),
            reply_to: reply_to_of(&parsed),
            is_at_me: parsed.is_at_me,
            send_state: send_state::CONFIRMED,
        };

        match state.db.tx(|c| message_store::upsert(c, &new_msg)) {
            Ok(fresh) => {
                if fresh {
                    inserted += 1;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "历史消息入库失败");
                continue;
            }
        }

        if parsed.has_image() {
            dispatch_images(app, state, &ev.message_id, &parsed);
        }
    }

    inserted
}

#[cfg(test)]
#[allow(uncommon_codepoints)]
mod tests {
    use super::*;
    use serde_json::json;

    const SELF: i64 = 10001;

    fn group_event() -> Value {
        json!({
            "post_type": "message",
            "message_type": "group",
            "sub_type": "normal",
            "message_id": 1234,
            "group_id": 30001,
            "user_id": 30011,
            "message_seq": 456,
            "time": 1_700_000_000,
            "self_id": SELF,
            "message": [
                { "type": "at", "data": { "qq": "10001" } },
                { "type": "text", "data": { "text": " 字段名定了没？" } }
            ],
            "sender": { "user_id": 30011, "nickname": "李工", "card": "李工(Li)" }
        })
    }

    #[test]
    fn 解析_群消息() {
        let ev = parse_message(&group_event(), SELF).unwrap();
        assert_eq!(ev.message_id, "1234");
        assert_eq!(ev.peer, Peer::group(30001));
        assert_eq!(ev.sender_id, 30011);
        assert_eq!(ev.sender_card.as_deref(), Some("李工(Li)"));
        assert_eq!(ev.sender_name().as_deref(), Some("李工(Li)"), "群名片优先于昵称");
        assert!(ev.parsed.is_at_me);
        assert_eq!(ev.message_seq, Some(456));
        assert!(!ev.is_self);
        assert_eq!(ev.ts, 1_700_000_000_000, "秒级时间戳被补成毫秒");
    }

    #[test]
    fn 解析_群名片缺失时退到昵称() {
        let mut v = group_event();
        v["sender"] = json!({ "user_id": 30011, "nickname": "李工" });
        let ev = parse_message(&v, SELF).unwrap();
        assert_eq!(ev.sender_name().as_deref(), Some("李工"));
    }

    #[test]
    fn 解析_自己发的消息被标记() {
        let mut v = group_event();
        v["sender"] = json!({ "user_id": SELF, "nickname": "我" });
        v["user_id"] = json!(SELF);
        let ev = parse_message(&v, SELF).unwrap();
        assert!(ev.is_self);
    }

    #[test]
    fn 解析_私聊() {
        let v = json!({
            "post_type": "message",
            "message_type": "private",
            "sub_type": "friend",
            "message_id": "s-9",
            "user_id": 20001,
            "time": 1_700_000_000_000_i64,
            "message": [{ "type": "text", "data": { "text": "在吗" } }],
            "sender": { "user_id": 20001, "nickname": "小陈" }
        });
        let ev = parse_message(&v, SELF).unwrap();
        assert_eq!(ev.peer, Peer::private(20001));
        assert_eq!(ev.temp_from_group, None);
        assert_eq!(ev.message_id, "s-9", "字符串 message_id 原样保留");
    }

    #[test]
    fn 解析_群临时会话() {
        let v = json!({
            "post_type": "message",
            "message_type": "private",
            "sub_type": "group",
            "message_id": 77,
            "user_id": 30011,
            "group_id": 30001,
            "time": 1_700_000_000,
            "message": [{ "type": "text", "data": { "text": "看到你群里说的了" } }],
            "sender": { "user_id": 30011, "nickname": "李工" }
        });
        let ev = parse_message(&v, SELF).unwrap();
        assert_eq!(ev.peer, Peer::private(30011), "临时会话仍算私聊");
        assert_eq!(ev.temp_from_group, Some(30001));
    }

    #[test]
    fn 解析_字符串形态的_message_不丢内容() {
        let v = json!({
            "post_type": "message",
            "message_type": "private",
            "message_id": 1,
            "user_id": 2,
            "message": "纯字符串消息",
            "sender": { "user_id": 2 }
        });
        let ev = parse_message(&v, SELF).unwrap();
        assert_eq!(ev.parsed.plain_text(), "纯字符串消息");
    }

    #[test]
    fn 解析_引用段被识别为_reply_to() {
        let v = json!({
            "post_type": "message",
            "message_type": "group",
            "message_id": 5,
            "group_id": 1,
            "user_id": 2,
            "message": [
                { "type": "reply", "data": { "id": 99 } },
                { "type": "text", "data": { "text": "收到" } }
            ],
            "sender": { "user_id": 2 }
        });
        let ev = parse_message(&v, SELF).unwrap();
        assert_eq!(reply_to_of(&ev.parsed).as_deref(), Some("99"));
    }

    #[test]
    fn 解析_缺少必需字段时返回空() {
        // 没有 user_id
        assert!(parse_message(&json!({ "post_type": "message", "message_id": 1 }), SELF).is_none());
        // 没有 message_id
        assert!(parse_message(
            &json!({ "post_type": "message", "message_type": "private", "user_id": 1 }),
            SELF
        )
        .is_none());
        // 不是消息事件
        assert!(parse_message(&json!({ "post_type": "notice" }), SELF).is_none());
        // message_id 是空的
        assert!(parse_message(
            &json!({ "post_type": "message", "user_id": 1, "message_id": "" }),
            SELF
        )
        .is_none());
    }

    #[test]
    fn 事件判定_响应不算事件() {
        assert!(is_event(&json!({ "post_type": "message" })));
        assert!(!is_event(&json!({ "post_type": "message", "echo": "1" })), "带 echo 的是响应");
        assert!(!is_event(&json!({ "retcode": 0 })));
    }

    #[test]
    fn 解析_群撤回() {
        let v = json!({
            "post_type": "notice",
            "notice_type": "group_recall",
            "group_id": 30001,
            "user_id": 30011,
            "operator_id": 30012,
            "message_id": 1234
        });
        let ev = parse_recall(&v).unwrap();
        assert_eq!(ev.peer, Peer::group(30001));
        assert_eq!(ev.message_id, "1234");
        assert_eq!(ev.operator_id, Some(30012));
    }

    #[test]
    fn 解析_好友撤回() {
        let v = json!({
            "post_type": "notice",
            "notice_type": "friend_recall",
            "user_id": 20001,
            "message_id": "abc"
        });
        let ev = parse_recall(&v).unwrap();
        assert_eq!(ev.peer, Peer::private(20001));
        assert_eq!(ev.message_id, "abc");
        assert_eq!(ev.operator_id, None);
    }

    #[test]
    fn 解析_非撤回的_notice_被忽略() {
        assert!(parse_recall(&json!({
            "post_type": "notice", "notice_type": "group_increase", "group_id": 1, "user_id": 2
        }))
        .is_none());
        assert!(parse_recall(&json!({ "post_type": "message", "message_id": 1 })).is_none());
    }

    #[test]
    fn 好友名_备注优先() {
        let data = json!({
            "list": [
                { "user_id": 2, "nickname": "小陈", "remark": "陈工" },
                { "user_id": 3, "nickname": "大林" }
            ]
        });
        assert_eq!(find_friend_name(&data, 2).as_deref(), Some("陈工"));
        assert_eq!(find_friend_name(&data, 3).as_deref(), Some("大林"));
        assert_eq!(find_friend_name(&data, 9), None);
    }

    #[test]
    fn 成员行_解析出四个字段() {
        let data = json!({
            "list": [
                { "user_id": 1, "nickname": "小李", "card": "", "role": "member" },
                { "nickname": "没有 id 的项" },
                { "user_id": 2, "nickname": "小陈", "card": "陈老师", "role": "admin" }
            ]
        });
        let rows = member_rows(&data);
        assert_eq!(rows.len(), 2, "没有 user_id 的项被跳过");
        assert_eq!(rows[0].0, 1);
        assert_eq!(rows[1].2.as_deref(), Some("陈老师"));
    }

    /* ---------- 历史记录：与实时事件共用同一套解析 ---------- */

    #[test]
    fn 历史_缺_post_type_时补上再解析() {
        // 这是 get_group_msg_history 常见形态：只有消息本体
        let v = json!({
            "message_id": 9001,
            "group_id": 30001,
            "user_id": 30011,
            "time": 1_700_000_000,
            "message": [{ "type": "text", "data": { "text": "翻到的历史" } }],
            "sender": { "user_id": 30011, "nickname": "李工" }
        });
        let ev = parse_history_item(&v, SELF).unwrap();
        assert_eq!(ev.message_id, "9001");
        assert_eq!(ev.peer, Peer::group(30001), "没有 message_type 时靠 group_id 判群聊");
        assert_eq!(ev.parsed.plain_text(), "翻到的历史");
    }

    #[test]
    fn 历史_带_post_type_时走同一条路径() {
        let ev = parse_history_item(&group_event(), SELF).unwrap();
        assert_eq!(ev.message_id, "1234");
    }

    #[test]
    fn 历史_私聊不带_group_id() {
        let v = json!({
            "message_id": 9002,
            "user_id": 20001,
            "time": 1_700_000_000_000_i64,
            "message": [{ "type": "text", "data": { "text": "私聊历史" } }],
            "sender": { "user_id": 20001, "nickname": "小陈" }
        });
        let ev = parse_history_item(&v, SELF).unwrap();
        assert_eq!(ev.peer, Peer::private(20001));
    }

    #[test]
    fn 历史_缺少_message_id_的坏记录被跳过() {
        assert!(parse_history_item(&json!({ "group_id": 1, "user_id": 2 }), SELF).is_none());
    }

    /* ---------- 端到端：上游事件 → 幂等入库 → 会话预览 → 墓碑过滤 ---------- */

    /// 这条测试跨了 `ob::event` / `store::message` / `store::conversation` / `store::tombstone`
    /// 四个模块。它盯的是**接缝**：单看每一层都对，拼起来却"删了又出现"这种情况
    /// 只有在真库里跑一遍才抓得到。
    #[test]
    fn 端到端_从上游事件到历史过滤() -> anyhow::Result<()> {
        use crate::store::{tombstone, Db};

        let db = Db::open_memory()?;
        let peer = Peer::group(30001);
        db.tx(|c| conv_store::ensure(c, peer, "大前端交流群", None))?;

        // 1) 解析上游事件
        let ev = parse_message(&group_event(), SELF).expect("事件应该能解析");
        assert!(ev.parsed.is_at_me, "群里的 @我 必须被识别");
        let preview = ev.parsed.plain_text();
        let sender = ev.sender_name();

        let nm = message_store::NewMessage {
            message_id: ev.message_id.clone(),
            peer,
            ts: ev.ts,
            sender_id: ev.sender_id,
            sender_name: sender.clone(),
            is_self: ev.is_self,
            segments: ev.parsed.segments.clone(),
            reply_to: reply_to_of(&ev.parsed),
            is_at_me: ev.parsed.is_at_me,
            send_state: send_state::CONFIRMED,
        };

        // 2) 幂等：事件投递一次、历史拉取又送一次，库里仍然只有一行
        assert!(db.tx(|c| message_store::upsert(c, &nm))?, "第一次写入是新消息");
        assert!(
            !db.tx(|c| message_store::upsert(c, &nm))?,
            "同一条 message_id 再写一次不算新消息（FR-22）"
        );
        assert_eq!(db.with(|c| message_store::count(c, peer))?, 1);

        // 3) 会话预览与未读/@ 标记
        db.tx(|c| conv_store::touch_last(c, peer, ev.ts, &preview, sender.as_deref(), true, true))?;
        let conv = db.with(|c| conv_store::get(c, peer))?.expect("会话应该在");
        assert_eq!(conv.unread_count, 1);
        assert!(conv.has_mention, "被 @ 过之后标记要留住");
        assert!(conv.last_msg_text.unwrap_or_default().contains("字段名"));
        assert_eq!(conv.last_msg_sender.as_deref(), Some("李工(Li)"));

        // 4) 手动删除 → 写墓碑 → 历史拉取必须看不见它
        db.tx(|c| {
            tombstone::add(c, &ev.message_id, peer, Some("测试"))?;
            message_store::delete(c, &ev.message_id)
        })?;
        let page = db.with(|c| message_store::list_page(c, peer, None, 30))?;
        assert!(page.is_empty(), "墓碑命中的消息不能被历史拉取送回来（FR-45）");
        assert!(!db.with(|c| message_store::has_older(c, peer, 999))?);

        // 5) 提醒决策与上面这条消息一致：非静音、没在当前会话、@我 → 闪 + 抢折叠条
        let intent = notify::decide(
            &NotifyInput::new(false, false)
                .with_at_me(true)
                .with_place(true, false),
        );
        assert!(intent.flash);
        assert_eq!(intent.mark, notify::mark::TAB, "在标签栏但非当前标签 → 留标签痕迹");
        assert!(intent.claim_bar);

        Ok(())
    }
}
