/**
 * Tauri 事件订阅（§4.2）。
 *
 * 前端只做两件事：把收到的快照/增量喂给 store，把用户意图翻译成 CSS 类。
 * 所有判定（该不该闪、该不该留痕、该不该抢折叠条）都在 Rust 侧完成。
 */

import * as ipc from './ipc';
import * as S from './store';

export async function initEvents(): Promise<() => void> {
  const unlisteners = await Promise.all([
    ipc.onConnState((info) => {
      S.setConn(info.state);
      S.setSelfId(info.self_id);
    }),

    ipc.onConversations((list) => S.setConversations(list)),

    ipc.onMessageAdded(({ message, conversation }) => {
      S.addMessage(message);
      // 会话快照跟着消息一起到，未读与预览由 Rust 算好——前端不做增量推断。
      // 快照可能缺失（Rust 取不到会话行），这时退化成"只收下消息"：
      // 未读数会在下一次 `conversations` 全量推送时补正，绝不能因此丢掉消息。
      if (conversation) S.upsertConversation(conversation);
    }),

    ipc.onMessageUpdated((msg) => S.updateMessage(msg)),

    ipc.onMessageRemoved((id) => S.removeMessage(id)),

    ipc.onNotify((intent) => S.applyNotify(intent)),

    ipc.onHistoryPage((page) => {
      S.prependMessages(
        { peer_type: page.peer_type, peer_id: page.peer_id },
        page.messages,
        page.at_top,
      );
      S.setHistoryLoading(false);
    }),

    /**
     * 换了 QQ 账号：Rust 已经把上个账号的会话/消息/图片清掉了，
     * 前端必须把消息缓存也丢掉，否则"新账号不在那个群里、旧会话却还在"。
     * 随后的 `conversations` 快照会用新账号的种子把界面填回来。
     */
    ipc.onAccountChanged((selfId) => {
      S.setSelfId(selfId);
      S.resetForAccount();
    }),

    // 失焦收起：Rust 先发事件让 UI 淡出，再改窗口尺寸（§4.3）
    ipc.onAutoCollapse(() => {
      if (!S.state.locked) requestCollapse();
    }),

    ipc.onTogglePanel(() => {
      if (S.state.expanded) requestCollapse();
      else void openPanel();
    }),

    // 托盘菜单的「设置…」「缓存管理…」：面板可能还是收起的，但 state.sheet 是持久的，
    // 所以先记下来，面板挂载后自然就显示出来了
    ipc.onOpenSheet((sheet) => {
      if (sheet === 'settings' || sheet === 'cache') S.setSheet(sheet);
    }),

    ipc.onToast((t) => S.showToast(t.text, t.kind ?? 'info')),
  ]);

  return () => {
    for (const un of unlisteners) un();
  };
}

/** 展开：先让 Rust 把窗口撑开，再取回消息 —— 加载历史不该拖住展开的手感 */
export async function openPanel(): Promise<void> {
  if (S.state.expanded) return;

  // 窗口几何由 Rust 掌握（§4.1）。这一步必须在 setExpanded 之前：
  // 顺序反了的话，内容会先按面板尺寸布局，而窗口还只有 40px 高。
  await ipc.expand();
  S.setExpanded(true);

  const conv = S.currentConversation();
  if (conv) {
    await ensureMessages(conv);
    // 展开即视为已读（FR-23 / FR-32）：Rust 清未读后会把新快照推回来
    await ipc.markRead(conv.peer_type, conv.peer_id);
  }
}

/** 收起动画时长（§3.8） */
const COLLAPSE_MS = 160;

/**
 * 收起：先播内容淡出，再让 Rust 把窗口尺寸一次到位。
 * 幂等——已在收起流程中时重复触发不产生副作用（§3.4 关键规则 5）。
 */
export function requestCollapse(): void {
  if (!S.state.expanded || S.state.closing) return;
  S.setClosing(true);
  window.setTimeout(() => {
    void ipc.collapse();
    S.setExpanded(false);
  }, COLLAPSE_MS);
}

/** 收起（同步版）：按钮点击用，行为与失焦一致 */
export function closePanel(): void {
  requestCollapse();
}

/** 保证某个会话已经有消息（首次进入时拉一页） */
export async function ensureMessages(conv: {
  peer_type: 0 | 1;
  peer_id: number;
}): Promise<void> {
  const key = `${conv.peer_type}:${conv.peer_id}`;
  if (S.state.messages[key] !== undefined) return;
  const page = await ipc.loadMessages(conv.peer_type, conv.peer_id, null, 30);
  S.setMessages(conv, page, page.length < 30);
}

/** 向上翻页（FR-20）：本地不够一页时由 Rust 决定是否向远端补 */
export async function loadOlder(): Promise<void> {
  const conv = S.currentConversation();
  if (conv === null || S.state.historyLoading) return;
  if (S.atTopOf(conv)) return;

  S.setHistoryLoading(true);
  const before = S.currentMessages()[0]?.seq ?? null;
  const page = await ipc.loadMessages(conv.peer_type, conv.peer_id, before, 30);
  S.prependMessages(conv, page, page.length < 30);
  S.setHistoryLoading(false);
}
