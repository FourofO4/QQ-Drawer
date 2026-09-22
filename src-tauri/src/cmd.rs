//! 暴露给前端的 Tauri 命令（§4.2）。
//!
//! 这里是**唯一的 IPC 契约面**：命令名与参数名必须和 `src/state/ipc.ts` 的 `CMD`
//! 逐字对上（Tauri 会把前端的 camelCase 参数映射到这里的 snake_case 形参）。
//!
//! 三条约定：
//!  1. **只返回 DTO**。SQLite 的行、OneBot 的报文一律不许从这里漏出去；
//!  2. **写操作一律经 `store`**，写完把新的快照推给前端（`emit_conversations`），
//!     不让前端自己推断增量；
//!  3. 前端传进来的东西一律当作**不可信输入**再校验一遍 —— 前端已经拦过一次，
//!     但那是体验，不是安全性（§4.7 发送前校验）。

use base64::Engine as _;
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

use crate::appstate::{emit, emit_conversations, events, toast, AppState};
use crate::model::{
    image_state, send_state, CacheOverviewDto, ConnInfo, ConversationDto, MemberDto, MessageDto,
    OutgoingPart, Peer, Seg, Settings,
};
use crate::ob::{self, segments};
use crate::store::{
    conversation as conv_store, image as image_store, member as member_store,
    message as message_store, mute as mute_store, now_ms, settings as settings_store, tombstone,
};
use crate::{hotkey, media, tray, window};

/// 命令的返回类型：错误一律转成可以直接展示给用户的字符串。
type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/* ------------------------------ 一次性返回的小结构 ------------------------------ */

#[derive(Clone, Debug, Serialize)]
pub struct SendAck {
    pub message_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PastedImage {
    pub sha256: String,
    pub path: String,
}

/* ------------------------------ 本地乐观条目的临时 id ------------------------------ */

static LOCAL_SEQ: AtomicU64 = AtomicU64::new(1);

/// 自己发的消息先落在本地，拿一个临时 id；`send_*` 成功后再换成上游的真实 id（FR-24）。
fn local_message_id() -> String {
    format!("local-{}-{}", now_ms(), LOCAL_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 输入层令牌 → 规范化消息段。
///
/// 这是"乐观条目能立刻渲染出正确内容"的关键：等上游回包之前，
/// 用户就该看到自己刚发的那条消息长什么样。
pub fn segments_of(parts: &[OutgoingPart], self_id: i64) -> Vec<Seg> {
    parts
        .iter()
        .map(|p| match p {
            OutgoingPart::Text { text } => Seg::Text { text: text.clone() },
            OutgoingPart::At { qq, name } => Seg::At {
                qq: *qq,
                name: Some(name.clone()).filter(|n| !n.trim().is_empty()),
                is_self: *qq == self_id && self_id != 0,
            },
            // 本地路径已经存在，所以直接就是 READY —— 不需要下载
            OutgoingPart::Image { path, .. } => Seg::Image {
                path: Some(path.clone()),
                sub_type: 0,
                state: image_state::READY,
            },
            OutgoingPart::Reply { id } => Seg::Reply { id: id.clone() },
        })
        .collect()
}

/// 消息段 → 输入令牌（重发用）。
///
/// 无法原样重发的段（占位段、没有本地路径的图片段）被丢掉 ——
/// 重发一条"表情 + 文字"的消息时只把文字发出去，比整条发失败体验好。
pub fn tokens_of(msg: &MessageDto) -> Vec<OutgoingPart> {
    msg.segments
        .iter()
        .filter_map(|s| match s {
            Seg::Text { text } if !text.is_empty() => {
                Some(OutgoingPart::Text { text: text.clone() })
            }
            Seg::At { qq, name, .. } => Some(OutgoingPart::At {
                qq: *qq,
                name: name.clone().unwrap_or_default(),
            }),
            Seg::Image { path: Some(p), .. } if !p.is_empty() => Some(OutgoingPart::Image {
                sha256: message_store::sha_from_path(p).unwrap_or_default(),
                path: p.clone(),
            }),
            Seg::Reply { id } => Some(OutgoingPart::Reply { id: id.clone() }),
            _ => None,
        })
        .collect()
}

/// 读一条消息并把「消息 + 该会话最新快照」推给前端。
///
/// `added = true` 走 `msg_added`（带会话快照，前端顺势更新未读与预览）；
/// 否则走 `msg_updated`（裸 DTO）。
fn emit_message(app: &AppHandle, state: &Arc<AppState>, message_id: &str, added: bool) {
    let Ok(Some(msg)) = state.db.with(|c| message_store::get(c, message_id)) else {
        return;
    };
    if added {
        let conv = state
            .db
            .with(|c| {
                conv_store::get(c, Peer { peer_type: msg.peer_type, peer_id: msg.peer_id })
            })
            .ok()
            .flatten();
        emit(
            app,
            events::MSG_ADDED,
            crate::model::MessageAddedPayload { message: msg, conversation: conv },
        );
    } else {
        emit(app, events::MSG_UPDATED, msg);
    }
}

/* ------------------------------ 连接 ------------------------------ */

#[tauri::command]
pub fn conn_status(state: State<'_, Arc<AppState>>) -> ConnInfo {
    state.conn_info()
}

/// 「点击重试」：唤醒 ws 循环立刻重连，而不是等退避走完（最长 30 秒）。
#[tauri::command]
pub fn retry_connect(app: AppHandle, state: State<'_, Arc<AppState>>) {
    state.set_conn(ConnInfo::connecting());
    emit(&app, events::CONN_STATE, state.conn_info());
    state.wake.notify_one();
}

/* ------------------------------ 会话 ------------------------------ */

#[tauri::command]
pub fn list_conversations(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<ConversationDto>> {
    state.db.with(conv_store::list).map_err(err)
}

#[tauri::command]
pub async fn refresh_conversations(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    count: Option<i64>,
) -> CmdResult<()> {
    let state = state.inner().clone();
    let bus = state.bus.read().clone().ok_or("还没有连接上 NapCat")?;
    let count = count.unwrap_or(ob::ws::RECENT_COUNT).clamp(1, 100);

    let data = bus.get_recent_contact(count).await.map_err(err)?;
    let seed = segments::recent_contact_seed(&ob::api::items_of(&data, &["list"]));
    if !seed.is_empty() {
        state.db.tx(|c| conv_store::seed_many(c, &seed)).map_err(err)?;
    }
    emit_conversations(&app, &state);
    Ok(())
}

/* ------------------------------ 消息 ------------------------------ */

/// 打开会话时读一页。本地不够一页就顺手向远端补一次（FR-20）。
#[tauri::command]
pub async fn load_messages(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    peer_type: i32,
    peer_id: i64,
    before_seq: Option<i64>,
    limit: Option<i64>,
) -> CmdResult<Vec<MessageDto>> {
    let state = state.inner().clone();
    let peer = Peer { peer_type, peer_id };
    let limit = limit.unwrap_or(30).clamp(1, 100);

    let local = state
        .db
        .with(|c| message_store::list_page(c, peer, before_seq, limit))
        .map_err(err)?;
    if local.len() >= limit as usize {
        return Ok(local);
    }

    // 本地不足一页 —— 说明这一页有一部分在远端（或者本地确实到底了）。
    let Some(bus) = state.bus.read().clone() else {
        return Ok(local);
    };
    let want = limit - local.len() as i64;
    if want <= 0 {
        return Ok(local);
    }

    // OneBot 的游标是上游 `message_seq`，与本地自增 `seq` 不是一回事。
    // 能从上一条本地消息的原始 id 里解析出数字就用它，否则退回「最新一页」；
    // 退化的后果只是多补一页重复数据，而写入是幂等的，不会产生重复消息。
    let cursor = local.first().and_then(|m| m.message_id.parse::<i64>().ok());
    match ob::ws::remote_history(&bus, peer, ob::ws::history_cursor(cursor), want).await {
        Ok(items) if !items.is_empty() => {
            let n = ob::event::ingest_history(&app, &state, state.self_id(), &items);
            tracing::debug!(peer = %peer.key(), added = n, "远端历史补齐");
        }
        Ok(_) => {}
        Err(e) => tracing::debug!(error = %e, "远端历史补齐失败，先用本地数据"),
    }

    state
        .db
        .with(|c| message_store::list_page(c, peer, before_seq, limit))
        .map_err(err)
}

/// 发送消息（§4.7）。
///
/// 顺序是刻意的：**先落本地乐观条目 → 再交给后台任务发上游 → 成功后把临时条目换成真实 id**。
/// 反过来（等上游回包再落库）会让"发送中"有一段时间是空白的，
/// 用户会以为按钮没反应而连点。
///
/// 注意**这里不等上游**：见下面第 2 步的注释。
#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    peer_type: i32,
    peer_id: i64,
    parts: Vec<OutgoingPart>,
) -> CmdResult<SendAck> {
    let state = state.inner().clone();
    let peer = Peer { peer_type, peer_id };
    segments::validate_outgoing(&parts, peer.is_group())?;

    // 没连上就别收下这条消息：否则会留下一条永远发不出去的乐观条目
    if state.bus.read().is_none() {
        return Err("还没有连接上 NapCat".into());
    }
    let self_id = state.self_id();
    let temp_id = local_message_id();

    // 1) 乐观条目
    let local = message_store::NewMessage {
        message_id: temp_id.clone(),
        peer,
        ts: now_ms(),
        sender_id: self_id,
        sender_name: Some("我".into()),
        is_self: true,
        segments: segments_of(&parts, self_id),
        reply_to: parts.iter().find_map(|p| match p {
            OutgoingPart::Reply { id } => Some(id.clone()),
            _ => None,
        }),
        is_at_me: false,
        send_state: send_state::LOCAL,
    };
    let preview = local.text_preview();
    state
        .db
        .tx(|c| {
            message_store::upsert(c, &local)?;
            // 自己发的不加未读、不标记 @我
            conv_store::touch_last(c, peer, local.ts, &preview, Some("我"), false, false)
        })
        .map_err(err)?;
    emit_message(&app, &state, &temp_id, true);

    // 2) 发上游：**命令不等结果**。
    //
    //    乐观条目已经落库并推给前端了，用户此刻已经看见自己发的内容 —— 所以"什么时候拿到
    //    上游回执"跟界面无关。而在这里 await 的代价很大：
    //      · 带图片的消息要先上传，超时按 `SEND_TIMEOUT_MS` 算是 30 秒，输入框会被 busy 卡住；
    //      · 更要命的是"超时"在旧实现里被当成"失败"，而上游其实还在跑，最后照样发了出去
    //        —— 于是报错、消息却出现了，还多一条重复。这正是要修的 bug。
    //    所以交给后台任务，命令立刻返回临时 id。
    tauri::async_runtime::spawn(finish_send(
        app.clone(),
        state.clone(),
        peer,
        temp_id.clone(),
        local,
        segments::build_outgoing(&parts),
    ));

    Ok(SendAck { message_id: temp_id })
}

/// 超时之后还等多久的回执，才退回"发送失败"。
///
/// 超时只说明我们没等到 action 响应，上游可能仍在发送 —— 这段时间就是留给
/// 那条 `message_sent` 回执的。等到了就无事发生，等不到才让用户看到失败（可重试）。
const ECHO_GRACE_MS: u64 = 60_000;

/// 把一条已落库的乐观消息真正交给上游，并按结果收敛它的状态。
///
/// 三种结果必须分开处理，这是"发图偶发报错但其实发出去了"的修复核心：
///  · 成功 → 换成上游的真实 id；
///  · **超时 → 不当失败**，保持"在途"，等回执来对账（`message_store::take_pending`），
///    过了 [`ECHO_GRACE_MS`] 还没等到才标失败；
///  · 其它错误 → 上游明确拒绝了这次发送，直接标失败。
async fn finish_send(
    app: AppHandle,
    state: Arc<AppState>,
    peer: Peer,
    temp_id: String,
    optimistic: message_store::NewMessage,
    message: Value,
) {
    let Some(bus) = state.bus.read().clone() else {
        mark_send_failed(&app, &state, &temp_id, "连接已断开");
        return;
    };

    let sent = if peer.is_group() {
        bus.send_group_msg(peer.peer_id, message).await
    } else {
        bus.send_private_msg(peer.peer_id, message).await
    };

    match sent {
        Ok(data) => {
            let real_id = ob::api::sent_message_id(&ob::api::ActionResponse {
                echo: None,
                ok: true,
                error: None,
                data,
            })
            .unwrap_or_else(|| temp_id.clone());
            replace_local(&app, &state, &temp_id, &real_id, optimistic);
        }

        Err(e) if ob::api::is_timeout(&e) => {
            tracing::warn!(
                message_id = %temp_id,
                timeout_ms = ob::api::SEND_TIMEOUT_MS,
                "发送超时：不当失败处理，转入等待回执（上游可能仍在发送）"
            );
            let app2 = app.clone();
            let state2 = state.clone();
            let tid = temp_id;
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(ECHO_GRACE_MS)).await;
                if is_still_pending(&state2, &tid) {
                    tracing::warn!(message_id = %tid, "等到回执超时，标记为发送失败");
                    mark_send_failed(&app2, &state2, &tid, "上游未在预期时间内确认");
                }
            });
        }

        Err(e) => {
            tracing::warn!(message_id = %temp_id, error = %e, "上游拒绝了这次发送");
            mark_send_failed(&app, &state, &temp_id, &e.to_string());
        }
    }
}

/// 乐观条目还在不在、且仍是"在途"。给超时后的宽限期用。
fn is_still_pending(state: &Arc<AppState>, message_id: &str) -> bool {
    state
        .db
        .with(|c| message_store::get(c, message_id))
        .ok()
        .flatten()
        .map(|m| m.send_state == send_state::LOCAL || m.send_state == send_state::SENDING)
        .unwrap_or(false)
}

/// 把乐观条目换成上游的真实 id。
///
/// **幂等**：`message_sent` 回执可能已经抢先做了对账（`take_pending` 把本地条目收掉了），
/// 那种情况下这里什么都不用做 —— 真实条目已经在库里了。
fn replace_local(
    app: &AppHandle,
    state: &Arc<AppState>,
    temp_id: &str,
    real_id: &str,
    optimistic: message_store::NewMessage,
) {
    let gone = state
        .db
        .with(|c| message_store::get(c, temp_id))
        .ok()
        .flatten()
        .is_none();
    if gone {
        tracing::debug!(temp_id, real_id, "本地条目已被回执对账收走，跳过替换");
        return;
    }

    // 先落真实条目再删临时条目：中间态里两条都在，比"两条都没有"好得多。
    let real = message_store::NewMessage {
        message_id: real_id.to_string(),
        send_state: send_state::CONFIRMED,
        ..optimistic
    };
    if let Err(e) = state.db.tx(|c| {
        message_store::upsert(c, &real)?;
        if real_id != temp_id {
            message_store::delete(c, temp_id)?;
        }
        Ok(())
    }) {
        tracing::warn!(error = %e, "替换乐观条目失败");
    }
    emit(app, events::MSG_REMOVED, temp_id.to_string());
    emit_message(app, state, real_id, false);
}

/// 把乐观条目标成失败态并推给前端（前端据此显示「发送失败 · 点击重试」）。
fn mark_send_failed(app: &AppHandle, state: &Arc<AppState>, message_id: &str, why: &str) {
    if !is_still_pending(state, message_id) {
        // 已经不在途了（成功换了 id、或已被回执收走）—— 别把一条好消息标成失败
        tracing::debug!(message_id, why, "条目已不在途，跳过失败标记");
        return;
    }
    if let Err(e) = state
        .db
        .tx(|c| message_store::set_send_state(c, message_id, send_state::FAILED))
    {
        tracing::warn!(error = %e, "标记发送失败状态失败");
    }
    emit_message(app, state, message_id, false);
}

/// 重发一条失败的本地消息（FR-24）。
#[tauri::command]
pub async fn retry_send(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    message_id: String,
) -> CmdResult<SendAck> {
    let state = state.inner().clone();
    let msg = state
        .db
        .with(|c| message_store::get(c, &message_id))
        .map_err(err)?
        .ok_or("这条消息已经不在本地了")?;

    if !msg.is_self {
        return Err("只能重发自己发出的消息".into());
    }
    if state.bus.read().is_none() {
        return Err("还没有连接上 NapCat".into());
    }

    let parts = tokens_of(&msg);
    if parts.is_empty() {
        return Err("这条消息没有可重发的内容".into());
    }
    let peer = Peer { peer_type: msg.peer_type, peer_id: msg.peer_id };
    segments::validate_outgoing(&parts, peer.is_group())?;

    if let Err(e) = state
        .db
        .tx(|c| message_store::set_send_state(c, &message_id, send_state::SENDING))
    {
        tracing::warn!(error = %e, "标记重发中失败");
    }
    emit_message(&app, &state, &message_id, false);

    // 与 `send_message` 同一条路径：不在命令里等上游，交给后台任务收敛状态。
    // 重发用的"乐观条目"就是原来那条（id 不变，只是要被换成上游真实 id）。
    let optimistic = message_store::NewMessage {
        message_id: message_id.clone(),
        peer,
        ts: msg.ts,
        sender_id: msg.sender_id,
        sender_name: msg.sender_name.clone(),
        is_self: true,
        segments: msg.segments.clone(),
        reply_to: msg.reply_to.clone(),
        is_at_me: false,
        send_state: send_state::SENDING,
    };
    tauri::async_runtime::spawn(finish_send(
        app.clone(),
        state.clone(),
        peer,
        message_id.clone(),
        optimistic,
        segments::build_outgoing(&parts),
    ));

    Ok(SendAck { message_id })
}

/* ------------------------------ 已读 ------------------------------ */

#[tauri::command]
pub fn mark_read(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    peer_type: i32,
    peer_id: i64,
) -> CmdResult<()> {
    let peer = Peer { peer_type, peer_id };
    state.db.tx(|c| conv_store::mark_read(c, peer)).map_err(err)?;

    // 顺手同步到 QQ（打开会话视为已读，FR-23 / FR-32）。失败不影响本地已读。
    if let Some(bus) = state.bus.read().clone() {
        if peer.is_group() {
            bus.mark_group_msg_as_read(peer_id);
        } else {
            bus.mark_private_msg_as_read(peer_id);
        }
    }
    emit_conversations(&app, state.inner());
    Ok(())
}

#[tauri::command]
pub fn mark_all_read(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.db.tx(|c| conv_store::reset_all_unread(c)).map_err(err)?;
    if let Some(bus) = state.bus.read().clone() {
        bus.mark_all_as_read();
    }
    emit_conversations(&app, state.inner());
    Ok(())
}

/* ------------------------------ 静音 ------------------------------ */

/// 只写本地库，**不调用任何 NapCat 接口**（FR-37）。
#[tauri::command]
pub fn set_mute(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    peer_type: i32,
    peer_id: i64,
    muted: bool,
) -> CmdResult<()> {
    let peer = Peer { peer_type, peer_id };
    state
        .db
        .tx(|c| mute_store::set(c, peer, muted, Some("用户手动切换")))
        .map_err(err)?;
    emit_conversations(&app, state.inner());
    Ok(())
}

/* ------------------------------ 删除与撤回 ------------------------------ */

/// **本地手动删除**：只写墓碑表，不上报 QQ（FR-45 / §4.8 #10）。
///
/// 顺手清理图片，但只在**没有别的消息引用这张图**时才删文件。
/// 图按 sha256 去重，同一张图可能被多条消息（甚至别的会话）引用，
/// 无脑删会让别处变成 `[图片已清除]`。
#[tauri::command]
pub fn delete_message(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    message_id: String,
) -> CmdResult<()> {
    let state_arc = state.inner().clone();
    let msg = state
        .db
        .with(|c| message_store::get(c, &message_id))
        .map_err(err)?
        .ok_or("这条消息已经不在本地了")?;

    let peer = Peer { peer_type: msg.peer_type, peer_id: msg.peer_id };
    let shas: Vec<String> = msg
        .segments
        .iter()
        .filter_map(|s| match s {
            Seg::Image { path: Some(p), .. } => message_store::sha_from_path(p),
            _ => None,
        })
        .collect();

    state
        .db
        .tx(|c| {
            tombstone::add(c, &message_id, peer, Some("用户手动删除"))?;
            message_store::delete(c, &message_id)
        })
        .map_err(err)?;

    for sha in shas {
        let others = state_arc
            .db
            .with(|c| image_store::referenced_by(c, &sha))
            .unwrap_or(0);
        if others == 0 {
            media::purge_shas(&state_arc, std::slice::from_ref(&sha));
        }
    }

    emit(&app, events::MSG_REMOVED, message_id);
    emit_conversations(&app, &state_arc);
    Ok(())
}

/// **撤回**自己发出的消息。会让消息在对方那边也消失，
/// 与本地手动删除完全不是一件事（§4.6）。
#[tauri::command]
pub async fn recall_message(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    message_id: String,
) -> CmdResult<()> {
    let state = state.inner().clone();
    let msg = state
        .db
        .with(|c| message_store::get(c, &message_id))
        .map_err(err)?
        .ok_or("这条消息已经不在本地了")?;

    if !msg.is_self {
        return Err("只能撤回自己发出的消息".into());
    }
    let bus = state.bus.read().clone().ok_or("还没有连接上 NapCat")?;
    bus.delete_msg(&message_id)
        .await
        .map_err(|e| format!("撤回失败：{e}"))?;

    state
        .db
        .tx(|c| message_store::mark_recalled(c, &message_id, Some(state.self_id())))
        .map_err(err)?;
    emit_message(&app, &state, &message_id, false);
    Ok(())
}

/* ------------------------------ 标签 ------------------------------ */

/// 手动加入标签栏：优先级最高，满了挤掉队尾的**非手动**标签（FR-13）。
#[tauri::command]
pub fn add_tab(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    peer_type: i32,
    peer_id: i64,
) -> CmdResult<()> {
    let peer = Peer { peer_type, peer_id };
    let limit = state.settings.read().tab_limit.clamp(1, 20);
    state
        .db
        .tx(|c| conv_store::push_tab(c, peer, true, limit))
        .map_err(err)?;
    emit_conversations(&app, state.inner());
    Ok(())
}

#[tauri::command]
pub fn remove_tab(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    peer_type: i32,
    peer_id: i64,
) -> CmdResult<()> {
    let peer = Peer { peer_type, peer_id };
    state.db.tx(|c| conv_store::remove_tab(c, peer)).map_err(err)?;
    emit_conversations(&app, state.inner());
    Ok(())
}

#[tauri::command]
pub fn reorder_tabs(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    order: Vec<String>,
) -> CmdResult<()> {
    state
        .db
        .tx(|c| conv_store::reorder_tabs(c, &order))
        .map_err(err)?;
    emit_conversations(&app, state.inner());
    Ok(())
}

/* ------------------------------ 成员 ------------------------------ */

/// @ 选人用的群成员列表。本地缓存 24h，过期或为空时才打上游（§4.8 #4）。
#[tauri::command]
pub async fn list_members(
    state: State<'_, Arc<AppState>>,
    group_id: i64,
    query: Option<String>,
) -> CmdResult<Vec<MemberDto>> {
    let state = state.inner().clone();
    let query = query.unwrap_or_default();

    let stale = state
        .db
        .with(|c| member_store::is_stale(c, group_id))
        .unwrap_or(true);

    // 先把 bus 克隆出来再 await：`state.bus.read()` 的读锁守卫不是 Send，
    // 一旦跨 await 持有，整个 future 就不是 Send，`#[tauri::command]` 直接编不过。
    let bus = state.bus.read().clone();

    if stale {
        if let Some(bus) = bus {
            match bus.get_group_member_list(group_id).await {
                Ok(data) => {
                    let rows = ob::event::member_rows(&data);
                    if !rows.is_empty() {
                        if let Err(e) = state.db.tx(|c| member_store::upsert_many(c, group_id, &rows))
                        {
                            tracing::warn!(error = %e, "写入群成员缓存失败");
                        }
                    }
                }
                // 拉不到就用旧缓存：@ 选人退化成只能按 QQ 号搜，总比空列表好
                Err(e) => tracing::warn!(error = %e, "拉取群成员失败"),
            }
        }
    }

    state
        .db
        .with(|c| member_store::list(c, group_id, &query, 200))
        .map_err(err)
}

/* ------------------------------ 缓存 ------------------------------ */

#[tauri::command]
pub fn cache_overview(state: State<'_, Arc<AppState>>) -> CmdResult<CacheOverviewDto> {
    let s = state.settings_snapshot();
    state
        .db
        .with(|c| image_store::overview(c, s.cache_keep_days, s.cache_limit_bytes))
        .map_err(err)
}

/// 「清空缓存」的三种口径（FR-42）：`peer` / `age` / `all`。
#[tauri::command]
pub fn clear_cache(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    scope: String,
    peer_type: Option<i32>,
    peer_id: Option<i64>,
    days: Option<i64>,
) -> CmdResult<()> {
    let state = state.inner().clone();
    let s = state.settings_snapshot();
    let peer = match (peer_type, peer_id) {
        (Some(t), Some(id)) => Some(Peer { peer_type: t, peer_id: id }),
        _ => None,
    };
    let report = media::clear(
        &state,
        &scope,
        peer,
        days.unwrap_or(0),
        s.cache_keep_days,
        s.cache_limit_bytes,
    );

    // 图片状态变了（段变成 `[图片已清除]`），让消息列表重绘
    if let Some(p) = peer {
        for m in state
            .db
            .with(|c| message_store::list_latest(c, p, 60))
            .unwrap_or_default()
        {
            emit(&app, events::MSG_UPDATED, m);
        }
    }

    toast(
        &app,
        &format!(
            "已清理 {} 张图片，释放 {} MB",
            report.removed,
            report.freed_bytes / 1024 / 1024
        ),
        "info",
    );
    Ok(())
}

/* ------------------------------ 设置 ------------------------------ */

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> Settings {
    state.settings_snapshot()
}

/// 只改一个键（FR-47）。未知键由 `store::settings::set` 忽略。
#[tauri::command]
pub fn set_setting(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    key: String,
    value: Value,
) -> CmdResult<()> {
    state
        .db
        .tx(|c| settings_store::set(c, &key, &value))
        .map_err(err)?;

    // 重新读一遍而不是就地改字段：这样"库里存了什么"永远是唯一事实
    let fresh = state.db.with(settings_store::get_all).map_err(err)?;
    *state.settings.write() = fresh.clone();

    apply_setting_side_effects(&app, state.inner(), &key, &fresh);
    Ok(())
}

/// 设置项落地后的即时副作用。
///
/// 抽出来是因为「改了一个键之后窗口该不该动」这件事很容易漏 ——
/// 比如改了折叠条宽度却没重算位置，折叠条右半截就会跑到屏幕外。
fn apply_setting_side_effects(app: &AppHandle, state: &Arc<AppState>, key: &str, s: &Settings) {
    match key {
        "always_on_top" => {
            if let Some(win) = app.get_webview_window("main") {
                if let Err(e) = win.set_always_on_top(s.always_on_top) {
                    tracing::warn!(error = %e, "设置置顶失败");
                }
            }
        }
        "bar_width" => {
            // 展开态下窗口宽度由面板决定，等收起时再重算
            if !state.expanded.load(Ordering::Relaxed) {
                window::on_bar_width_changed(app, state);
            }
        }
        "locked" => {
            tray::sync_lock_check(app, s.locked);
            if s.locked {
                // 刚锁定：撤掉"等会儿要自动收起"的待办，否则会锁完就收
                window::suppress_auto_collapse(state, window::SUPPRESS_MS);
            }
        }
        "hotkey_toggle" | "hotkey_mute" => {
            if let Err(e) = hotkey::register(app, state) {
                tracing::warn!(error = %e, "重新注册快捷键失败");
            }
        }
        _ => {}
    }
}

/// 粘贴的图片先落盘换一个本地绝对路径（FR-26）。
///
/// 发送时只把这个路径交给 Rust —— 同机运行，WebSocket 只传几十字节，
/// 几 MB 的截图不会撑爆连接，也避免 base64 进内存造成的尖峰（§4.6）。
#[tauri::command]
pub fn save_pasted_image(
    state: State<'_, Arc<AppState>>,
    data_url: String,
) -> CmdResult<PastedImage> {
    let (mime, bytes) = decode_data_url(&data_url)?;
    let ext = media::guess_ext("", Some(&mime));
    let (sha, abs, rel) = media::store_bytes(&state.media_root, &bytes, &ext).map_err(err)?;

    let (w, h) = match media::sniff_size(&bytes) {
        Some((w, h)) => (Some(w), Some(h)),
        None => (None, None),
    };
    if let Err(e) = state
        .db
        .tx(|c| image_store::register(c, &sha, &rel, &ext, bytes.len() as i64, w, h, 0))
    {
        tracing::warn!(error = %e, "粘贴图片登记失败");
    }

    Ok(PastedImage { sha256: sha, path: abs })
}

/// 拆 `data:image/png;base64,xxxx`。
///
/// 只认 base64 形态 —— 别的形态（`data:...;charset=utf-8,`）在前端就已经被转成 blob 了。
pub fn decode_data_url(data_url: &str) -> CmdResult<(String, Vec<u8>)> {
    let rest = data_url.strip_prefix("data:").ok_or("不是 data URL")?;
    let (meta, payload) = rest.split_once(',').ok_or("data URL 缺少逗号分隔符")?;

    let mime = meta
        .split(';')
        .next()
        .filter(|m| m.starts_with("image/"))
        .ok_or("只支持图片")?
        .to_string();
    if !meta.contains("base64") {
        return Err("只支持 base64 编码的 data URL".into());
    }

    // 换行不算数据（某些来源会按行折行）
    let cleaned: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| format!("图片数据解码失败：{e}"))?;

    if bytes.is_empty() {
        return Err("剪贴板里没有图片".into());
    }
    if bytes.len() > media::MAX_IMAGE_BYTES {
        return Err(format!(
            "图片太大了（{} MB），上限 {} MB",
            bytes.len() / 1024 / 1024,
            media::MAX_IMAGE_BYTES / 1024 / 1024
        ));
    }
    Ok((mime, bytes))
}

/* ------------------------------ 窗口 ------------------------------ */

#[tauri::command]
pub fn expand_window(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    window::set_expanded(&app, state.inner(), true).map_err(err)
}

#[tauri::command]
pub fn collapse_window(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    window::set_expanded(&app, state.inner(), false).map_err(err)
}

/// 拖动窗口。**不要直接调前端的 `startDragging()`** —— 那会绕过"拖动期间抑制自动收起"，
/// 未锁定时拖动会把自己收起来（见 `window::begin_drag`）。
#[tauri::command]
pub fn begin_drag(app: AppHandle, state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    window::begin_drag(&app, state.inner()).map_err(err)
}

/// 问一次窗口形态的权威值。
///
/// 前端在 bootstrap 时对齐一次，避免"窗口已经是展开尺寸、界面却还画着折叠条"
/// 这种状态在没有切换动作的情况下一直挂着。
#[tauri::command]
pub fn window_state(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> CmdResult<crate::model::WindowStateDto> {
    let expanded = state.inner().expanded.load(std::sync::atomic::Ordering::Relaxed);
    let (width, height) = state.inner().size_for(expanded);
    let actual = app
        .get_webview_window("main")
        .and_then(|w| w.outer_size().map_err(|e| e.to_string()).ok())
        .map(|s| (s.width, s.height))
        .unwrap_or((width, height));
    tracing::info!(
        expanded,
        expect_w = width,
        expect_h = height,
        actual_w = actual.0,
        actual_h = actual.1,
        "窗口形态自检"
    );
    Ok(crate::model::WindowStateDto {
        expanded,
        width: actual.0,
        height: actual.1,
    })
}

/// 前端同步「我正在看哪个会话」。
///
/// 提醒决策（该不该闪、要不要即时已读）完全依赖它 ——
/// 没有这一步，用户盯着某个会话时每来一条消息都会闪（FR-31）。
#[tauri::command]
pub fn set_viewing(
    state: State<'_, Arc<AppState>>,
    peer_type: Option<i32>,
    peer_id: Option<i64>,
) -> CmdResult<()> {
    match (peer_type, peer_id) {
        (Some(t), Some(id)) => state.set_viewing(Some(Peer { peer_type: t, peer_id: id })),
        _ => state.set_viewing(None),
    }
    Ok(())
}

#[tauri::command]
pub fn exit_app(app: AppHandle) {
    tracing::info!("用户请求退出");
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_message(segments: Vec<Seg>) -> MessageDto {
        MessageDto {
            message_id: "1".into(),
            peer_type: 1,
            peer_id: 30001,
            seq: Some(1),
            ts: 1,
            sender_id: 10001,
            sender_name: Some("我".into()),
            is_self: true,
            text: Some("来了".into()),
            segments,
            reply_to: None,
            reply_preview: None,
            is_at_me: false,
            has_image: false,
            image_state: image_state::NONE,
            images: vec![],
            is_recalled: false,
            recalled_by: None,
            recalled_by_name: None,
            send_state: send_state::LOCAL,
        }
    }

    #[test]
    fn 令牌转消息段() {
        let parts = vec![
            OutgoingPart::Reply { id: "9".into() },
            OutgoingPart::Text { text: "来了 ".into() },
            OutgoingPart::At { qq: 30011, name: "李工".into() },
            OutgoingPart::Image { sha256: "x".into(), path: "C:/m/a.png".into() },
        ];
        let segs = segments_of(&parts, 10001);
        assert_eq!(segs.len(), 4);
        assert_eq!(segs[0], Seg::Reply { id: "9".into() });
        assert_eq!(segs[1], Seg::Text { text: "来了 ".into() });
        assert_eq!(
            segs[2],
            Seg::At { qq: 30011, name: Some("李工".into()), is_self: false }
        );
        assert_eq!(
            segs[3],
            Seg::Image {
                path: Some("C:/m/a.png".into()),
                sub_type: 0,
                state: image_state::READY
            },
            "本地已有的图片直接就是已落盘状态"
        );
    }

    #[test]
    fn 艾特自己时_is_self_为真() {
        let segs = segments_of(&[OutgoingPart::At { qq: 10001, name: "我".into() }], 10001);
        match &segs[0] {
            Seg::At { is_self, .. } => assert!(is_self),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn self_id_未知时不会误判艾特自己() {
        let segs = segments_of(&[OutgoingPart::At { qq: 10001, name: "我".into() }], 0);
        match &segs[0] {
            Seg::At { is_self, .. } => assert!(!is_self),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 艾特名字为空时降级为_none() {
        let segs = segments_of(&[OutgoingPart::At { qq: 2, name: "  ".into() }], 1);
        match &segs[0] {
            Seg::At { name, .. } => assert_eq!(*name, None),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 反向转换_把本地条目还原成令牌() {
        let msg = sample_message(vec![
            Seg::Text { text: "来了 ".into() },
            Seg::Image {
                path: Some(
                    "C:/m/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.png"
                        .into(),
                ),
                sub_type: 0,
                state: image_state::READY,
            },
            // 占位段与没有路径的图片段都还原不了
            Seg::placeholder("record", "[语音]"),
            Seg::Image { path: None, sub_type: 0, state: image_state::FAILED },
        ]);
        let parts = tokens_of(&msg);
        assert_eq!(parts.len(), 2, "占位段与无路径的图片段被丢掉");
        match &parts[1] {
            OutgoingPart::Image { sha256, path } => {
                assert_eq!(
                    sha256,
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                );
                assert!(path.ends_with(".png"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 反向转换_空文本段不产出令牌() {
        let msg = sample_message(vec![Seg::Text { text: String::new() }]);
        assert!(tokens_of(&msg).is_empty());
    }

    #[test]
    fn 临时条目_id_不重复() {
        let a = local_message_id();
        let b = local_message_id();
        assert_ne!(a, b);
        assert!(a.starts_with("local-"));
    }

    #[test]
    fn 解码_data_url_正常路径() {
        // "hi" 的 base64 是 aGk=
        let (mime, bytes) = decode_data_url("data:image/png;base64,aGk=").unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, b"hi");
    }

    #[test]
    fn 解码_data_url_容忍换行() {
        let (_, bytes) = decode_data_url("data:image/jpeg;base64,aG\nk=").unwrap();
        assert_eq!(bytes, b"hi");
    }

    #[test]
    fn 解码_data_url_拒绝非图片与非_base64() {
        assert!(decode_data_url("data:text/plain;base64,aGk=").is_err());
        assert!(decode_data_url("data:image/png,raw").is_err());
        assert!(decode_data_url("http://x/a.png").is_err());
        assert!(decode_data_url("data:image/png").is_err());
    }

    #[test]
    fn 解码_data_url_空内容被拒() {
        assert!(decode_data_url("data:image/png;base64,").is_err());
    }

    #[test]
    fn 解码_data_url_坏_base64_被拒() {
        assert!(decode_data_url("data:image/png;base64,!!!!").is_err());
    }
}
