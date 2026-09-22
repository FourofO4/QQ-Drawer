//! 账号归属：登录后确认"现在是谁在登录"，换账号时清掉上一个账号的本地数据。
//!
//! 修复的现象：换 QQ 账号后，上个账号的会话与聊天记录仍在，默认标签还停在旧会话上
//! —— 哪怕新账号根本不在那个群里。
//!
//! 三条设计约束：
//!  1. **判定必须有持久化的依据**。老版本从来没把 `self_id` 写进 meta
//!     （`lib.rs` 读它、却没人写它），所以只看内存值永远判不出切换；
//!     这里补上落库，并额外处理"从没记过"的那一次（见 `store::account::account_switched`）。
//!  2. **清的是账号数据，不是用户配置**。`settings` 一律不动，换号不该让用户重填
//!     ws_url / token / 窗口位置。
//!  3. **清完要让前端彻底忘掉旧消息**。只推会话快照不够——前端消息缓存按 peerKey 存，
//!     两个账号在同一个群里时 peerKey 相同，旧消息会冒充成新账号的消息。
//!     所以额外发一个 `ACCOUNT_CHANGED`。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::AppHandle;

use crate::appstate::{emit, emit_conversations, events, toast, AppState};
use crate::store::account as account_store;
use crate::store::schema;

/// meta 里记录"上次登录的是谁"的键。
pub const SELF_ID_KEY: &str = "self_id";

/// 连接后拿到 `self_id` 时调用（`ob::ws::session`）。
///
/// 返回 `Some(_)` 表示识别到账号切换、并且已经清空本地数据；
/// `None` 表示同账号（或拿不到 self_id），什么都没做。
pub fn apply_login(
    app: &AppHandle,
    state: &Arc<AppState>,
    self_id: i64,
) -> Option<account_store::ResetReport> {
    if self_id <= 0 {
        // 拿不到就别动：宁可这轮不判，也不能因为 0 把好数据清了
        tracing::warn!("拿不到 self_id，跳过账号归属检查");
        return None;
    }

    let prev = state
        .db
        .with(|c| schema::meta_get(c, SELF_ID_KEY))
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);

    // 先落库再判：这次写进去，下次才知道"上次是谁"。
    if let Err(e) = state
        .db
        .tx(|c| schema::meta_set(c, SELF_ID_KEY, &self_id.to_string()))
    {
        tracing::warn!(error = %e, "self_id 落库失败（下次仍判不出切换）");
    }
    state.self_id.store(self_id, Ordering::Relaxed);

    let has_data = state
        .db
        .with(account_store::has_local_data)
        .unwrap_or(false);

    if !account_store::account_switched(prev, has_data, self_id) {
        return None;
    }

    let report = match state.db.tx(|c| account_store::reset(c)) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "清空上个账号的本地数据失败");
            return None;
        }
    };

    // 图片是上个账号的消息带出来的，消息没了它们就是孤儿，一起清掉。
    // 注意这是"清全部图片"，与用户手动清空缓存的 `all` 同一个口径。
    let images = crate::media::clear(state, "all", None, 0, 0, 0);

    // 内存里的账号态也要跟着清：正在看的会话、本次运行期间提醒过的会话
    state.set_viewing(None);
    state.notified.write().clear();

    tracing::warn!(
        prev,
        self_id,
        rows = report.total(),
        images = images.removed,
        "检测到换了 QQ 账号，已清空上个账号的本地数据"
    );

    emit(app, events::ACCOUNT_CHANGED, self_id);
    emit_conversations(app, state);
    toast(app, "检测到换了 QQ 账号，已清空上一个账号的本地记录", "info");

    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Settings;
    use crate::store::Db;
    use std::path::PathBuf;

    /// 造一个带数据的库，并把 meta 里"上次登录"设成 `prev`。
    fn state_with(prev: i64) -> Arc<AppState> {
        let db = Db::open_memory().unwrap();
        db.tx(|c| {
            c.execute(
                "INSERT INTO conversation(peer_type, peer_id, name, unread_count, has_mention,
                                          is_manual_tab, updated_at)
                 VALUES(1, 777, '旧账号的群', 3, 1, 1, 1)",
                [],
            )?;
            c.execute(
                "INSERT INTO message(message_id, peer_type, peer_id, ts, sender_id, segments,
                                     seg_types, created_at)
                 VALUES('old-1', 1, 777, 1, 9, '[]', 'text', 1)",
                [],
            )?;
            if prev > 0 {
                schema::meta_set(c, SELF_ID_KEY, &prev.to_string())?;
            }
            Ok(())
        })
        .unwrap();
        AppState::new(db, Settings::default(), PathBuf::from("."))
    }

    fn rows(state: &Arc<AppState>) -> (i64, i64) {
        state
            .db
            .with(|c| {
                Ok((
                    c.query_row("SELECT COUNT(*) FROM conversation", [], |r| r.get(0))?,
                    c.query_row("SELECT COUNT(*) FROM message", [], |r| r.get(0))?,
                ))
            })
            .unwrap()
    }

    fn meta_self_id(state: &Arc<AppState>) -> Option<String> {
        state.db.with(|c| schema::meta_get(c, SELF_ID_KEY)).unwrap()
    }

    #[test]
    fn 同账号重连不清数据() {
        let st = state_with(1001);
        let prev = meta_self_id(&st).unwrap().parse::<i64>().unwrap();
        let has_data = st.db.with(account_store::has_local_data).unwrap();
        assert!(
            !account_store::account_switched(prev, has_data, 1001),
            "同一个号重连不该清掉本地记录"
        );
        assert_eq!(rows(&st), (1, 1), "没判成切换就不该有人动过库");
    }

    #[test]
    fn 老库没有self_id时视为换账号() {
        // 模拟升级：库里没有 self_id 键，但有上一个账号的数据
        let st = state_with(0);
        assert!(meta_self_id(&st).is_none(), "前置条件：老库确实没记过 self_id");
        let has_data = st.db.with(account_store::has_local_data).unwrap();
        assert!(has_data);
        assert!(
            account_store::account_switched(0, has_data, 2002),
            "没依据 + 有数据 → 必须当作上个账号的残留清掉，否则 bug 修不掉"
        );
    }
}
