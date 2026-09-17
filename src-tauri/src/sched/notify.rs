//! 提醒决策（§3.5）。
//!
//! **全部是纯函数**——输入一组布尔量，输出该不该闪、痕迹留哪儿、抢不抢折叠条。
//! 这里刻意不碰数据库、不碰窗口、不碰事件系统：决策错了只需要改一个函数加几条断言。
//!
//! 三条容易搞错的规则，写在这里当文档：
//!  1. **静音是承诺**：静音会话不闪、不留痕（除非用户显式打开"被 @ 时仍然闪烁"）；
//!  2. 正在查看的会话来消息**一律不闪**，无论静音与否；
//!  3. 静音会话永远抢不到折叠条位置——只要存在未读的非静音会话。

use crate::model::{ConversationDto, NotifyIntent};

/// 痕迹出现的位置
pub mod mark {
    pub const NONE: &str = "none";
    pub const BAR: &str = "bar";
    pub const TAB: &str = "tab";
    pub const OVERFLOW: &str = "overflow";
}

/// 决策输入。字段都取自"这条消息 + 会话所在位置 + 用户设置"，不含时间。
#[derive(Clone, Copy, Debug)]
pub struct NotifyInput {
    /// 该会话是不是当前正在看的
    pub viewing: bool,
    /// 该会话是否在本地静音表里
    pub muted: bool,
    /// 这条消息是否 @ 了我
    pub at_me: bool,
    /// 该会话是否已放进标签栏
    pub in_tabs: bool,
    /// 在标签栏里的话，是不是当前选中的那个标签
    pub tab_is_current: bool,
    /// 设置项：静音会话被 @ 时是否仍然闪烁
    pub flash_on_mention_when_muted: bool,
}

impl NotifyInput {
    pub fn new(viewing: bool, muted: bool) -> Self {
        Self {
            viewing,
            muted,
            at_me: false,
            in_tabs: false,
            tab_is_current: false,
            flash_on_mention_when_muted: false,
        }
    }

    pub fn with_at_me(mut self, v: bool) -> Self {
        self.at_me = v;
        self
    }

    pub fn with_place(mut self, in_tabs: bool, tab_is_current: bool) -> Self {
        self.in_tabs = in_tabs;
        self.tab_is_current = tab_is_current;
        self
    }

    pub fn with_muted_mention_flash(mut self, v: bool) -> Self {
        self.flash_on_mention_when_muted = v;
        self
    }
}

/// 决策：该不该闪、痕迹留哪儿、抢不抢折叠条。
pub fn decide(input: &NotifyInput) -> NotifyIntent {
    let flash = should_flash(input);
    let trace = trace_mark(input);
    NotifyIntent {
        peer_type: 0,
        peer_id: 0,
        flash,
        mark: trace.to_string(),
        // 静音会话的折叠条内容只在"没有未读的非静音会话"时才可能显示，
        // 那个判定是全局的，交给 pick_bar()；单条消息只能表态到这一层。
        claim_bar: !input.muted,
    }
}

fn should_flash(input: &NotifyInput) -> bool {
    // 正在看这个会话：无论静音与否都不闪（FR-31）
    if input.viewing {
        return false;
    }
    if input.muted {
        // 尊重静音承诺；除非用户显式要求"被 @ 时仍然闪烁"
        return input.at_me && input.flash_on_mention_when_muted;
    }
    true
}

fn trace_mark(input: &NotifyInput) -> &'static str {
    if input.viewing {
        return mark::NONE;
    }
    if input.muted {
        return mark::NONE;
    }
    if !input.in_tabs {
        mark::OVERFLOW
    } else if input.tab_is_current {
        mark::BAR
    } else {
        mark::TAB
    }
}

/// 折叠条优先级（FR-33）：
///   ① 有未读的**非静音**会话 → 取最新的一条
///   ② 没有则未读的静音会话 → 取最新的一条
///   ③ 都没有未读 → 显示全局最新的一条
///
/// 返回 `None` 表示折叠条上无内容可显示（还没有任何消息）。
pub fn pick_bar(conversations: &[ConversationDto]) -> Option<&ConversationDto> {
    let with_time: Vec<&ConversationDto> =
        conversations.iter().filter(|c| c.last_msg_time.is_some()).collect();
    if with_time.is_empty() {
        return None;
    }

    let hot: Vec<&&ConversationDto> = with_time
        .iter()
        .filter(|c| c.unread_count > 0 && !c.is_muted)
        .collect();
    if !hot.is_empty() {
        return newest(hot.into_iter().copied());
    }

    let any_unread: Vec<&&ConversationDto> =
        with_time.iter().filter(|c| c.unread_count > 0).collect();
    if !any_unread.is_empty() {
        return newest(any_unread.into_iter().copied());
    }

    newest(with_time.into_iter())
}

fn newest<'a>(items: impl Iterator<Item = &'a ConversationDto>) -> Option<&'a ConversationDto> {
    let mut best: Option<&ConversationDto> = None;
    for c in items {
        let t = c.last_msg_time.unwrap_or(0);
        match best {
            None => best = Some(c),
            Some(b) if t > b.last_msg_time.unwrap_or(0) => best = Some(c),
            _ => {}
        }
    }
    best
}

/// 折叠条上的内容：`会话名：内容`。群聊带上发送者（`群名 · 老王：`）。
pub fn bar_text(conv: &ConversationDto) -> (String, String) {
    let name = if conv.is_group() {
        match &conv.last_msg_sender {
            Some(s) if !s.trim().is_empty() && s != &conv.name => format!("{} · {}", conv.name, s),
            _ => conv.name.clone(),
        }
    } else {
        conv.name.clone()
    };
    let body = conv.last_msg_text.clone().unwrap_or_default();
    (format!("{}：", name), body)
}

impl ConversationDto {
    pub fn is_group(&self) -> bool {
        self.peer_type == crate::model::PEER_GROUP
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PEER_GROUP;

    fn conv(peer_id: i64, over: impl FnOnce(&mut ConversationDto)) -> ConversationDto {
        let mut c = ConversationDto {
            peer_type: PEER_GROUP,
            peer_id,
            name: format!("会话{peer_id}"),
            last_msg_time: Some(1000),
            last_msg_text: Some("内容".into()),
            last_msg_sender: Some("李工".into()),
            unread_count: 0,
            has_mention: false,
            tab_order: None,
            is_manual_tab: false,
            is_muted: false,
        };
        over(&mut c);
        c
    }

    /* ---------- 策略矩阵逐行对照（§3.5） ---------- */

    #[test]
    fn 非静音且非当前会话_闪并留痕并抢折叠条() {
        let i = NotifyInput::new(false, false);
        let d = decide(&i);
        assert!(d.flash);
        assert_ne!(d.mark, mark::NONE);
        assert!(d.claim_bar);
    }

    #[test]
    fn 非静音但当前会话_不闪不留痕不抢() {
        let i = NotifyInput::new(true, false);
        let d = decide(&i);
        assert!(!d.flash);
        assert_eq!(d.mark, mark::NONE);
    }

    #[test]
    fn 静音_不闪不留痕_也抢不到折叠条() {
        let i = NotifyInput::new(false, true);
        let d = decide(&i);
        assert!(!d.flash);
        assert_eq!(d.mark, mark::NONE);
        assert!(!d.claim_bar);
    }

    #[test]
    fn 静音且被艾特_默认依旧不闪() {
        let i = NotifyInput::new(false, true).with_at_me(true);
        assert!(!decide(&i).flash);
    }

    #[test]
    fn 静音且被艾特_打开设置后可以闪() {
        let i = NotifyInput::new(false, true).with_at_me(true).with_muted_mention_flash(true);
        assert!(decide(&i).flash);
        // 但依然不留痕：静音就是静音
        assert_eq!(decide(&i).mark, mark::NONE);
    }

    #[test]
    fn 在标签栏但不是当前标签_痕迹落在标签上() {
        let i = NotifyInput::new(false, false).with_place(true, false);
        assert_eq!(decide(&i).mark, mark::TAB);
    }

    #[test]
    fn 在_···_里的会话_痕迹落在溢出区() {
        let i = NotifyInput::new(false, false).with_place(false, false);
        assert_eq!(decide(&i).mark, mark::OVERFLOW);
    }

    #[test]
    fn 当前标签收到消息_痕迹回到折叠条() {
        let i = NotifyInput::new(false, false).with_place(true, true);
        assert_eq!(decide(&i).mark, mark::BAR);
    }

    /* ---------- 折叠条优先级（FR-33） ---------- */

    #[test]
    fn 没有会话时无内容可显示() {
        assert!(pick_bar(&[]).is_none());
    }

    #[test]
    fn 还没有消息的会话不参与竞争() {
        let list = vec![conv(1, |c| c.last_msg_time = None)];
        assert!(pick_bar(&list).is_none());
    }

    #[test]
    fn 优先抢未读的非静音会话且取最新() {
        let list = vec![
            conv(1, |c| {
                c.unread_count = 1;
                c.last_msg_time = Some(5000);
            }),
            conv(2, |c| {
                c.unread_count = 1;
                c.last_msg_time = Some(9000);
            }),
            conv(3, |c| {
                c.unread_count = 9;
                c.is_muted = true;
                c.last_msg_time = Some(99999);
            }),
        ];
        assert_eq!(pick_bar(&list).unwrap().peer_id, 2);
    }

    #[test]
    fn 静音会话永远抢不到折叠条位置() {
        let list = vec![
            conv(3, |c| {
                c.unread_count = 9;
                c.is_muted = true;
                c.last_msg_time = Some(99999);
            }),
            conv(4, |c| {
                c.unread_count = 1;
                c.last_msg_time = Some(1);
            }),
        ];
        assert_eq!(pick_bar(&list).unwrap().peer_id, 4);
    }

    #[test]
    fn 没有非静音未读时让静音未读上折叠条() {
        let list = vec![
            conv(3, |c| {
                c.unread_count = 2;
                c.is_muted = true;
                c.last_msg_time = Some(7000);
            }),
            conv(4, |c| c.last_msg_time = Some(9000)),
        ];
        assert_eq!(pick_bar(&list).unwrap().peer_id, 3);
    }

    #[test]
    fn 完全没有未读时显示全局最新() {
        // 一条未读都没有 → 退化成「谁的 last_msg_time 最大就显示谁」。
        // conv() 默认给 1000，所以这里 1 号（7000）比 2 号（默认 1000）新。
        let list = vec![conv(1, |c| c.last_msg_time = Some(7000)), conv(2, |_| {})];
        assert_eq!(pick_bar(&list).unwrap().peer_id, 1);
    }

    /* ---------- 折叠条文案 ---------- */

    #[test]
    fn 私聊不拼发送者() {
        let c = conv(1, |c| {
            c.peer_type = crate::model::PEER_PRIVATE;
            c.name = "小陈".into();
            c.last_msg_text = Some("今天几点到？".into());
            c.last_msg_sender = Some("小陈".into());
        });
        assert_eq!(bar_text(&c), ("小陈：".into(), "今天几点到？".into()));
    }

    #[test]
    fn 群聊拼上发送者() {
        let c = conv(2, |c| {
            c.name = "产品组".into();
            c.last_msg_sender = Some("老王".into());
            c.last_msg_text = Some("下周再评审吧".into());
        });
        assert_eq!(bar_text(&c), ("产品组 · 老王：".into(), "下周再评审吧".into()));
    }

    #[test]
    fn 群名与发送者同名时不重复拼() {
        let c = conv(3, |c| {
            c.name = "读书会".into();
            c.last_msg_sender = Some("读书会".into());
        });
        assert_eq!(bar_text(&c).0, "读书会：");
    }
}
