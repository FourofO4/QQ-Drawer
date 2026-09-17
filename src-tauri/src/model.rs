//! 规范模型（DTO）。
//!
//! 这是 Rust 与前端之间的唯一契约，也是 `ob` 层之外**唯一**允许出现的数据形态。
//! OneBot 的字段名、事件名、消息段格式一律不许越界到这里（NFR-13）。
//!
//! 所有字段名与 `src/state/types.ts` 一一对应，改名两边一起改。

use serde::{Deserialize, Serialize};

pub const PEER_PRIVATE: i32 = 0;
pub const PEER_GROUP: i32 = 1;

/* ------------------------------ 会话标识 ------------------------------ */

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Peer {
    pub peer_type: i32,
    pub peer_id: i64,
}

impl Peer {
    pub fn private(user_id: i64) -> Self {
        Self { peer_type: PEER_PRIVATE, peer_id: user_id }
    }
    pub fn group(group_id: i64) -> Self {
        Self { peer_type: PEER_GROUP, peer_id: group_id }
    }
    pub fn is_group(&self) -> bool {
        self.peer_type == PEER_GROUP
    }
    /// 前端的 peerKey 形式 `"1:30001"`（标签排序用）
    pub fn key(&self) -> String {
        format!("{}:{}", self.peer_type, self.peer_id)
    }
    pub fn parse_key(key: &str) -> Option<Self> {
        let (t, id) = key.split_once(':')?;
        Some(Self { peer_type: t.parse().ok()?, peer_id: id.parse().ok()? })
    }
}

/* ------------------------------ 消息段 ------------------------------ */

/// 图片落盘状态：0=无 1=已落盘 2=下载中 3=失败 4=已清理
pub mod image_state {
    pub const NONE: i32 = 0;
    pub const READY: i32 = 1;
    pub const DOWNLOADING: i32 = 2;
    pub const FAILED: i32 = 3;
    pub const CLEANED: i32 = 4;
}

/// 发送状态：0=已确认 1=发送中 2=失败 3=本地乐观条目
pub mod send_state {
    pub const CONFIRMED: i32 = 0;
    pub const SENDING: i32 = 1;
    pub const FAILED: i32 = 2;
    pub const LOCAL: i32 = 3;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Seg {
    Text {
        text: String,
    },
    Image {
        /// 本地绝对路径；未落盘时为 None
        path: Option<String>,
        sub_type: i32,
        state: i32,
    },
    At {
        qq: i64,
        name: Option<String>,
        is_self: bool,
    },
    Reply {
        id: String,
    },
    Placeholder {
        kind: String,
        text: String,
    },
    System {
        text: String,
    },
}

impl Seg {
    pub fn text(s: impl Into<String>) -> Self {
        Seg::Text { text: s.into() }
    }

    pub fn placeholder(kind: &str, text: &str) -> Self {
        Seg::Placeholder { kind: kind.to_string(), text: text.to_string() }
    }

    /// 纯文本合并结果（搜索与折叠条预览用，§4.4 的 `message.text`）
    pub fn as_plain(&self) -> String {
        match self {
            Seg::Text { text } => text.clone(),
            Seg::Image { state, .. } => match *state {
                image_state::CLEANED => "[图片已清除]".into(),
                _ => "[图片]".into(),
            },
            Seg::At { name, qq, is_self } => {
                if *is_self {
                    "@你".into()
                } else {
                    format!("@{}", name.clone().unwrap_or_else(|| qq.to_string()))
                }
            }
            Seg::Reply { .. } => String::new(),
            Seg::Placeholder { text, .. } => text.clone(),
            Seg::System { text } => text.clone(),
        }
    }
}

/// 段类型串，如 `"text,image,at"`（入 message.seg_types，便于排查与统计）
pub fn seg_types(segs: &[Seg]) -> String {
    segs.iter()
        .map(|s| match s {
            Seg::Text { .. } => "text",
            Seg::Image { .. } => "image",
            Seg::At { .. } => "at",
            Seg::Reply { .. } => "reply",
            Seg::Placeholder { kind, .. } => kind.as_str(),
            Seg::System { .. } => "system",
        })
        .collect::<Vec<_>>()
        .join(",")
}

/* ------------------------------ 消息与会话 ------------------------------ */

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageRef {
    pub sha256: String,
    /// 相对 media 目录的路径，前端拼成 `http://media.localhost/<路径>` 后再喂给 `<img>`
    pub rel_path: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub sub_type: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplyPreview {
    pub message_id: String,
    pub sender_name: Option<String>,
    pub summary: String,
    /// 被引用消息已删除（墓碑命中）
    pub deleted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageDto {
    pub message_id: String,
    pub peer_type: i32,
    pub peer_id: i64,
    pub seq: Option<i64>,
    pub ts: i64,
    pub sender_id: i64,
    pub sender_name: Option<String>,
    pub is_self: bool,
    pub text: Option<String>,
    pub segments: Vec<Seg>,
    pub reply_to: Option<String>,
    pub reply_preview: Option<ReplyPreview>,
    pub is_at_me: bool,
    pub has_image: bool,
    pub image_state: i32,
    pub images: Vec<ImageRef>,
    pub is_recalled: bool,
    pub recalled_by: Option<i64>,
    pub recalled_by_name: Option<String>,
    pub send_state: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationDto {
    pub peer_type: i32,
    pub peer_id: i64,
    pub name: String,
    pub last_msg_time: Option<i64>,
    pub last_msg_text: Option<String>,
    pub last_msg_sender: Option<String>,
    pub unread_count: i64,
    pub has_mention: bool,
    /// None = 不在标签栏
    pub tab_order: Option<i64>,
    pub is_manual_tab: bool,
    pub is_muted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemberDto {
    pub user_id: i64,
    pub nickname: Option<String>,
    pub card: Option<String>,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheStatDto {
    pub peer_type: i32,
    pub peer_id: i64,
    pub name: String,
    pub bytes: i64,
    pub count: i64,
    pub oldest: Option<i64>,
    pub newest: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheOverviewDto {
    pub total_bytes: i64,
    pub total_count: i64,
    pub limit_bytes: i64,
    pub keep_days: i64,
    pub groups: Vec<CacheStatDto>,
}

/* ------------------------------ 连接与提醒 ------------------------------ */

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnInfo {
    /// connecting / connected / disconnected
    pub state: String,
    pub self_id: Option<i64>,
}

impl ConnInfo {
    pub fn connecting() -> Self {
        Self { state: "connecting".into(), self_id: None }
    }
    pub fn connected(self_id: i64) -> Self {
        Self { state: "connected".into(), self_id: Some(self_id) }
    }
    pub fn disconnected(self_id: Option<i64>) -> Self {
        Self { state: "disconnected".into(), self_id }
    }
}

/// 提醒决策结果（§3.5）。`sched::notify` 是唯一产出它的地方。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NotifyIntent {
    pub peer_type: i32,
    pub peer_id: i64,
    /// 折叠条是否要闪烁
    pub flash: bool,
    /// 痕迹留在哪里：none / bar / tab / overflow
    pub mark: String,
    /// 是否抢占折叠条内容
    pub claim_bar: bool,
}

/* ------------------------------ 事件负载 ------------------------------ */

/// `msg_added` / `msg_updated` 的负载：消息 + 该会话的最新快照。
/// 带上快照是为了让"消息到达 → 界面更新 ≤ 150 ms"不必回拉整个列表（NFR-04）。
#[derive(Clone, Debug, Serialize)]
pub struct MessageAddedPayload {
    pub message: MessageDto,
    pub conversation: Option<ConversationDto>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HistoryPagePayload {
    pub peer_type: i32,
    pub peer_id: i64,
    pub messages: Vec<MessageDto>,
    /// 远端也没有更早的了
    pub at_top: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToastPayload {
    pub text: String,
    pub kind: String,
}

/* ------------------------------ 设置 ------------------------------ */

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub ws_url: String,
    pub access_token: String,
    pub auto_reconnect: bool,

    pub bar_width: i32,
    pub always_on_top: bool,
    pub locked: bool,
    pub snap_top: bool,

    pub panel_alpha: f64,
    pub bubble_alpha: f64,
    pub readability_compensation: bool,
    pub motion: bool,

    pub tab_limit: i64,
    pub flash_times: i32,
    pub flash_period_ms: i32,
    pub flash_on_mention_when_muted: bool,

    pub cache_keep_days: i64,
    pub cache_limit_bytes: i64,
    pub cache_clean_on_start: bool,

    pub hotkey_toggle: String,
    pub hotkey_mute: String,
    pub log_level: String,

    /// 窗口位置，None 表示还没定过位（首次启动要算默认位置）
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ws_url: "ws://127.0.0.1:3001".into(),
            access_token: String::new(),
            auto_reconnect: true,

            bar_width: 264,
            always_on_top: true,
            locked: false,
            snap_top: true,

            panel_alpha: 0.72,
            bubble_alpha: 0.35,
            readability_compensation: true,
            motion: true,

            tab_limit: 5,
            flash_times: 3,
            flash_period_ms: 600,
            flash_on_mention_when_muted: false,

            cache_keep_days: 30,
            cache_limit_bytes: 2 * 1024 * 1024 * 1024,
            cache_clean_on_start: false,

            hotkey_toggle: "Ctrl+Alt+Q".into(),
            hotkey_mute: "Ctrl+Alt+M".into(),
            log_level: "info".into(),

            window_x: None,
            window_y: None,
        }
    }
}

/* ------------------------------ 发送入参 ------------------------------ */

/// 输入层的令牌（§4.7 发送）。
/// 前端只传这个中间形态，转成 OneBot 消息段的事在 `ob::segments` 里做。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutgoingPart {
    Text { text: String },
    At { qq: i64, name: String },
    Image { sha256: String, path: String },
    /// 引用：转成 reply 段放在最前
    Reply { id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_key_roundtrip() {
        let p = Peer::group(30001);
        assert_eq!(p.key(), "1:30001");
        assert_eq!(Peer::parse_key("1:30001"), Some(p));
        assert_eq!(Peer::parse_key("垃圾"), None);
    }

    #[test]
    fn seg_type_tag_matches_frontend_contract() {
        let json = serde_json::to_value(Seg::text("hi")).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hi");

        let json = serde_json::to_value(Seg::Reply { id: "9".into() }).unwrap();
        assert_eq!(json["type"], "reply");

        let json =
            serde_json::to_value(Seg::placeholder("face", "[表情]")).unwrap();
        assert_eq!(json["type"], "placeholder");
        assert_eq!(json["kind"], "face");
    }

    #[test]
    fn seg_types_join() {
        let segs = vec![
            Seg::text("a"),
            Seg::Image { path: None, sub_type: 0, state: 0 },
            Seg::At { qq: 1, name: None, is_self: false },
        ];
        assert_eq!(seg_types(&segs), "text,image,at");
    }

    #[test]
    fn plain_text_merges_segments() {
        let segs = vec![
            Seg::At { qq: 1, name: Some("李工".into()), is_self: false },
            Seg::text(" 看下"),
            Seg::Image { path: None, sub_type: 0, state: image_state::READY },
        ];
        let text: String = segs.iter().map(|s| s.as_plain()).collect();
        assert_eq!(text, "@李工 看下[图片]");
    }

    #[test]
    fn cleaned_image_shows_placeholder() {
        let seg = Seg::Image { path: None, sub_type: 0, state: image_state::CLEANED };
        assert_eq!(seg.as_plain(), "[图片已清除]");
    }

    #[test]
    fn settings_are_serializable_for_frontend() {
        let s = Settings::default();
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["ws_url"], "ws://127.0.0.1:3001");
        assert_eq!(json["panel_alpha"], 0.72);
        assert_eq!(json["tab_limit"], 5);
    }
}
