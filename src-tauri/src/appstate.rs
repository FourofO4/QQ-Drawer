//! 共享状态与前端事件出口。
//!
//! 说明：规格书的目录树（§4.2）没有单列这个文件，但 `AppState` 与"往前端发事件的唯一出口"
//! 需要一个明确的落点——散在 cmd/window/ob 里会让状态的所有权变模糊。
//! 这里的原则是：**谁都能读，写入必须走这里的方法**。

use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::model::{Peer, Settings};
use crate::store::Db;

/// 前端事件名。改动必须同步 `src/state/ipc.ts` 里的 `EV`（前端集成测试会盯着它）。
pub mod events {
    pub const CONN_STATE: &str = "conn_state";
    pub const CONVERSATIONS: &str = "conversations";
    pub const MSG_ADDED: &str = "msg_added";
    pub const MSG_UPDATED: &str = "msg_updated";
    pub const MSG_REMOVED: &str = "msg_removed";
    pub const NOTIFY: &str = "notify";
    pub const HISTORY_PAGE: &str = "history_page";
    /// 登录的 QQ 号与上次不同：本地已清空，前端必须**立刻丢掉全部消息缓存**。
    /// 载荷是新的 self_id（i64）。
    ///
    /// 光发 `CONVERSATIONS` 不够——前端 `state.messages` 是按 peerKey 存的，
    /// 新旧账号若在同一个群里，peerKey 相同，旧消息会冒充成新账号的消息显示出来。
    pub const ACCOUNT_CHANGED: &str = "account_changed";
    pub const AUTO_COLLAPSE: &str = "auto_collapse";
    pub const TOGGLE_PANEL: &str = "toggle_panel";
    /// 请求前端打开某个浮层（设置页 / 缓存管理页）。托盘菜单用。
    /// 载荷是 `"settings" | "cache"` 字符串。
    pub const OPEN_SHEET: &str = "open_sheet";
    pub const TOAST: &str = "toast";
}

/// 折叠条尺寸（§3.1）
pub const BAR_H: u32 = 40;
/// 展开面板尺寸（§3.1）
pub const PANEL_W: u32 = 584;
pub const PANEL_H: u32 = 500;
/// 越界判定的安全边距
pub const MARGIN: i32 = 16;

pub struct AppState {
    pub db: Db,
    pub settings: RwLock<Settings>,
    /// 自己发的消息的判定依据；连接成功后立即写入
    pub self_id: AtomicI64,
    /// 当前正在查看的会话（前端切会话时同步过来）
    pub viewing: RwLock<Option<Peer>>,
    /// 面板是否展开（决定"当前会话即时已读"）
    pub expanded: AtomicBool,
    /// 失焦收起的抑制窗口：托盘菜单交互前先设上，避免点托盘把面板关了（§3.4 关键规则 6）
    pub suppress_collapse_until: AtomicI64,
    /// 公告板：被提醒过的会话（仅本次运行期间有效，FR-39 重启即清空）
    pub notified: RwLock<Vec<Peer>>,
    /// 贴图缓存的落盘目录
    pub media_root: PathBuf,
    /// 数据目录
    pub data_root: PathBuf,
    /// 连接状态
    pub conn: RwLock<crate::model::ConnInfo>,
    /// 动作通道（连上之后才有）
    pub bus: RwLock<Option<crate::ob::api::ActionBus>>,
    /// 「立即重连」信号。
    ///
    /// 折叠条上的「点击重试」不能只是把状态改回 `connecting` 就完事——退避最长要等 30 秒，
    /// 用户点了没反应等于坏了。`retry_connect` 唤醒这个 `Notify`，ws 循环立刻中断等待重连。
    pub wake: tokio::sync::Notify,
}

impl AppState {
    pub fn new(db: Db, settings: Settings, data_root: PathBuf) -> Arc<Self> {
        let media_root = data_root.join("media");
        Arc::new(Self {
            db,
            settings: RwLock::new(settings),
            self_id: AtomicI64::new(0),
            viewing: RwLock::new(None),
            expanded: AtomicBool::new(false),
            suppress_collapse_until: AtomicI64::new(0),
            notified: RwLock::new(Vec::new()),
            media_root,
            data_root,
            conn: RwLock::new(crate::model::ConnInfo::connecting()),
            bus: RwLock::new(None),
            wake: tokio::sync::Notify::new(),
        })
    }

    pub fn self_id(&self) -> i64 {
        self.self_id.load(Ordering::Relaxed)
    }

    pub fn settings_snapshot(&self) -> Settings {
        self.settings.read().clone()
    }

    pub fn is_viewing(&self, peer: Peer) -> bool {
        if !self.expanded.load(Ordering::Relaxed) {
            return false;
        }
        self.viewing.read().map(|p| p == peer).unwrap_or(false)
    }

    pub fn set_viewing(&self, peer: Option<Peer>) {
        *self.viewing.write() = peer;
    }

    pub fn conn_info(&self) -> crate::model::ConnInfo {
        self.conn.read().clone()
    }

    pub fn set_conn(&self, info: crate::model::ConnInfo) {
        *self.conn.write() = info;
    }

    /// 折叠条 / 面板尺寸：两态各自固定，切换时**一次到位**（§3.4 关键规则 3）
    pub fn size_for(&self, expanded: bool) -> (u32, u32) {
        if expanded {
            (PANEL_W, PANEL_H)
        } else {
            let w = self.settings.read().bar_width.clamp(180, 420) as u32;
            (w, BAR_H)
        }
    }
}

/// 往前端发事件的唯一出口。所有 `emit` 都经过它，事件名才能在测试里被数出来。
pub fn emit<T: serde::Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    if let Err(e) = app.emit(event, payload) {
        tracing::warn!(event, error = %e, "事件发送失败");
    }
}

/// 会话快照变化：整表推给前端。
///
/// 几十条数据，全量刷新比做增量同步便宜得多，也不会出现两边不一致的诡异 bug。
pub fn emit_conversations(app: &AppHandle, state: &Arc<AppState>) {
    match state.db.with(crate::store::conversation::list) {
        Ok(list) => emit_with(app, events::CONVERSATIONS, list),
        Err(e) => tracing::warn!(error = %e, "读取会话列表失败"),
    }
}

fn emit_with<T: serde::Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    emit(app, event, payload)
}

/// 提示条（§3.10 的短文案，如「剪贴板里没有图片」）
pub fn toast(app: &AppHandle, text: &str, kind: &str) {
    emit(
        app,
        events::TOAST,
        crate::model::ToastPayload { text: text.to_string(), kind: kind.to_string() },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 尺寸在两态之间切换() {
        let db = Db::open_memory().unwrap();
        let st = AppState::new(db, Settings::default(), PathBuf::from("."));
        assert_eq!(st.size_for(false), (264, 40));
        assert_eq!(st.size_for(true), (584, 500));
    }

    #[test]
    fn 折叠条宽度被夹在合法区间() {
        let db = Db::open_memory().unwrap();
        let s = Settings { bar_width: 10_000, ..Settings::default() };
        let st = AppState::new(db, s, PathBuf::from("."));
        assert_eq!(st.size_for(false).0, 420);
    }

    #[test]
    fn 没展开时不算正在查看() {
        let db = Db::open_memory().unwrap();
        let st = AppState::new(db, Settings::default(), PathBuf::from("."));
        let p = Peer::group(1);
        st.set_viewing(Some(p));
        assert!(!st.is_viewing(p), "折叠态下无论如何都不算正在查看");
        st.expanded.store(true, Ordering::Relaxed);
        assert!(st.is_viewing(p));
        assert!(!st.is_viewing(Peer::group(2)));
    }

    #[test]
    fn 媒体目录挂在数据目录下() {
        let db = Db::open_memory().unwrap();
        let st = AppState::new(db, Settings::default(), PathBuf::from("C:/d/qq-drawer"));
        assert!(st.media_root.ends_with("media"));
    }
}
