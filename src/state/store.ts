/**
 * 前端视图状态。
 *
 * 这里**只有视图状态**：展开与否、当前选中谁、消息列表、闪烁与痕迹、弹层开关。
 * 业务状态的权威副本在 Rust 的 SQLite（§4.1 单一数据源），前端不缓存、不推断，
 * 收到的 `conversations` 快照直接替换即可。
 */

import { createRoot, createMemo, createSignal } from 'solid-js';
import { createStore, produce } from 'solid-js/store';
import type {
  ConnectionState,
  ConversationDTO,
  MemberDTO,
  MessageDTO,
  NotifyIntent,
  PeerRef,
  PeerType,
  SettingsDTO,
} from './types';
import { peerKey } from '../core/preview';
import { buildRows, type MessageRow } from '../core/grouping';
import * as ipc from './ipc';

export interface Toast {
  id: number;
  text: string;
  kind: 'info' | 'error';
}

/** 正在引用回复的目标；输入区上方会显示一条可取消的引用条 */
export interface Quote {
  message_id: string;
  sender_name: string | null;
  summary: string;
  deleted: boolean;
}

interface AppState {
  conn: ConnectionState;
  /** 自己的 QQ 号，连接成功后由 Rust 告知 */
  selfId: number | null;
  conversations: ConversationDTO[];
  /** 当前查看的会话，peerKey 形式；null 表示没有 */
  current: string | null;
  /** peerKey → 消息（ts 升序） */
  messages: Record<string, MessageDTO[]>;
  /** peerKey → 是否已经没有更早的历史 */
  atTop: Record<string, boolean>;
  /** 折叠条正在闪烁的会话（纯 UI 瞬时状态） */
  flashKey: string | null;
  expanded: boolean;
  /** 正在播放收起的淡出动画（§3.8 收起 160ms），动画结束后才真正改窗口尺寸 */
  closing: boolean;
  locked: boolean;
  overflowOpen: boolean;
  settings: SettingsDTO | null;
  historyLoading: boolean;
  members: MemberDTO[];
  toast: Toast | null;
  /** 图片放大查看 */
  viewer: string | null;
  /** 覆盖式弹层：设置 / 缓存管理；null 表示都没开 */
  sheet: 'settings' | 'cache' | null;
  /** 正在引用回复的目标 */
  quote: Quote | null;
}

const [state, setState] = createStore<AppState>({
  conn: 'connecting',
  selfId: null,
  conversations: [],
  current: null,
  messages: {},
  atTop: {},
  flashKey: null,
  expanded: false,
  closing: false,
  locked: false,
  overflowOpen: false,
  settings: null,
  historyLoading: false,
  members: [],
  toast: null,
  viewer: null,
  sheet: null,
  quote: null,
});

export { state };

/* ------------------------------ 派生视图 ------------------------------ */

/**
 * 「今天/昨天」的判定基准。刻意只在启动时取一次——时间分隔用静态文本，
 * 不做每秒刷新的相对时间，否则空闲 CPU 永远回不到 0（踩坑 #11）。
 */
export const [now] = createSignal(Date.now());

/**
 * 全局单例 store，所以派生量统一挂在 createRoot 下创建，
 * 避免 Solid 报「computations created outside a createRoot will never be disposed」。
 */
const derived = createRoot(() => {
  /** 标签栏：顺序**固定不自动重排**（FR-11），所以只按 tab_order 排 */
  const tabs = createMemo<ConversationDTO[]>(() =>
    state.conversations
      .filter((c) => c.tab_order !== null)
      .sort((a, b) => (a.tab_order ?? 0) - (b.tab_order ?? 0)),
  );

  /** `···` 列表：按最近消息时间倒序 */
  const overflowList = createMemo<ConversationDTO[]>(() =>
    state.conversations
      .filter((c) => c.tab_order === null)
      .sort((a, b) => (b.last_msg_time ?? 0) - (a.last_msg_time ?? 0)),
  );

  const currentConversation = createMemo<ConversationDTO | null>(() => {
    const key = state.current;
    if (key === null) return null;
    return state.conversations.find((c) => peerKey(c) === key) ?? null;
  });

  const currentMessages = createMemo<MessageDTO[]>(() => {
    const key = state.current;
    return key === null ? [] : (state.messages[key] ?? []);
  });

  const currentRows = createMemo<MessageRow[]>(() => buildRows(currentMessages(), now()));

  const canSendNow = createMemo(() => {
    const conv = currentConversation();
    return conv !== null && state.conn === 'connected';
  });

  /** 当前会话是否已经翻到最顶（决定要不要再请求更早的一页） */
  const currentAtTop = createMemo(() => {
    const key = state.current;
    return key !== null && state.atTop[key] === true;
  });

  return {
    tabs,
    overflowList,
    currentConversation,
    currentMessages,
    currentRows,
    canSendNow,
    currentAtTop,
  };
});

export const tabs = derived.tabs;
export const overflowList = derived.overflowList;
export const currentConversation = derived.currentConversation;
export const currentMessages = derived.currentMessages;
export const currentRows = derived.currentRows;
export const canSendNow = derived.canSendNow;
export const currentAtTop = derived.currentAtTop;

/* ------------------------------ 会话与消息 ------------------------------ */

export function setConn(conn: ConnectionState): void {
  setState('conn', conn);
}

export function setSelfId(id: number | null): void {
  setState('selfId', id);
}

/**
 * 「换账号后需要重挑一个默认会话」的一次性标记。
 * 放在模块级而不是 store 里：它是流程状态，不是要渲染的视图状态。
 */
let autoSelectPending = false;

/**
 * 整表替换会话快照。数据量是几十条，全量刷新比做增量同步便宜得多，也不易出错。
 * 注意：新会话追加到标签末尾这条规则由 Rust 决定，前端不插手。
 */
export function setConversations(list: ConversationDTO[]): void {
  setState('conversations', list);
  if (state.current !== null && !list.some((c) => peerKey(c) === state.current)) {
    setState('current', null);
  }
  // 换账号后需要重挑一个默认会话，等新账号的种子到齐再挑（见 resetForAccount）。
  // 只在这一个场景下自动选中：平时"选中项消失就保持不选"是有意义的语义
  // —— `applyNotify` 靠 `current` 判断"是不是正在看"，乱选会吃掉新消息的闪动提醒。
  if (autoSelectPending && list.length > 0) {
    autoSelectPending = false;
    const first = tabs()[0] ?? list[0];
    if (first !== undefined) selectConversation(first);
  }
}

/**
 * 换了 QQ 账号（Rust 已清空本地库，事件载荷是新 self_id）。
 *
 * 必须**连消息缓存一起丢**：消息按 peerKey 存，两个账号都在同一个群里时 peerKey
 * 完全相同，只清会话列表的话，旧账号的消息会冒充成新账号的消息显示出来。
 */
export function resetForAccount(): void {
  autoSelectPending = true;
  setState({
    conversations: [],
    messages: {},
    atTop: {},
    current: null,
    flashKey: null,
    quote: null,
    members: [],
    historyLoading: false,
    viewer: null,
    overflowOpen: false,
  });
}

export function upsertConversation(conv: ConversationDTO): void {
  setState(
    produce((s) => {
      const i = s.conversations.findIndex((c) => peerKey(c) === peerKey(conv));
      if (i >= 0) s.conversations[i] = conv;
      else s.conversations.push(conv);
    }),
  );
}

function keyOf(p: PeerRef): string {
  return peerKey(p);
}

/** 消息去重写入：`message_id` 幂等（FR-22），同一 id 只更新不追加 */
function mergeMessage(list: MessageDTO[], msg: MessageDTO): MessageDTO[] {
  const i = list.findIndex((m) => m.message_id === msg.message_id);
  if (i >= 0) {
    const next = [...list];
    next[i] = msg;
    return next;
  }
  // 按 ts 插入，保持升序；ts 相同按 seq 兜底
  const next = [...list, msg];
  next.sort((a, b) => a.ts - b.ts || (a.seq ?? 0) - (b.seq ?? 0));
  return next;
}

export function addMessage(msg: MessageDTO): void {
  const key = keyOf(msg);
  setState(
    produce((s) => {
      s.messages[key] = mergeMessage(s.messages[key] ?? [], msg);
    }),
  );
}

export function updateMessage(msg: MessageDTO): void {
  addMessage(msg);
}

/** 乐观条目被真实 message_id 取代时，把临时条目摘掉（FR-24） */
export function removeMessage(messageId: string): void {
  setState(
    produce((s) => {
      for (const key of Object.keys(s.messages)) {
        const list = s.messages[key];
        if (!list) continue;
        const i = list.findIndex((m) => m.message_id === messageId);
        if (i >= 0) s.messages[key] = [...list.slice(0, i), ...list.slice(i + 1)];
      }
    }),
  );
}

export function setMessages(peer: PeerRef, list: MessageDTO[], atTop: boolean): void {
  const key = keyOf(peer);
  const sorted = [...list].sort((a, b) => a.ts - b.ts || (a.seq ?? 0) - (b.seq ?? 0));
  setState(produce((s) => {
    s.messages[key] = sorted;
    s.atTop[key] = atTop;
  }));
}

/** 向上翻页：把新一页插到前面 */
export function prependMessages(peer: PeerRef, list: MessageDTO[], atTop: boolean): void {
  const key = keyOf(peer);
  setState(
    produce((s) => {
      let cur = s.messages[key] ?? [];
      for (const m of list) cur = mergeMessage(cur, m);
      cur.sort((a, b) => a.ts - b.ts || (a.seq ?? 0) - (b.seq ?? 0));
      s.messages[key] = cur;
      s.atTop[key] = atTop;
    }),
  );
}

export function setHistoryLoading(v: boolean): void {
  setState('historyLoading', v);
}

export function atTopOf(peer: PeerRef): boolean {
  return state.atTop[keyOf(peer)] === true;
}

/* ------------------------------ 窗口与选中 ------------------------------ */

export function setExpanded(v: boolean): void {
  setState('expanded', v);
  setState('closing', false);
  if (!v) setState('overflowOpen', false);
}

export function setClosing(v: boolean): void {
  setState('closing', v);
}

export function setLocked(v: boolean): void {
  setState('locked', v);
}

export function toggleLocked(): void {
  setState('locked', (v) => !v);
}

export function setOverflowOpen(v: boolean): void {
  setState('overflowOpen', v);
}

export function toggleOverflow(): void {
  setState('overflowOpen', (v) => !v);
}

/** 切到某个会话：置当前 + 通知 Rust 标已读（FR-23 / FR-32，痕迹随快照一起清掉） */
export function selectConversation(peer: PeerRef): void {
  setState('current', keyOf(peer));
  void ipc.markRead(peer.peer_type, peer.peer_id);
}

export function selectByKey(key: string | null): void {
  if (key === null) {
    setState('current', null);
    return;
  }
  const conv = state.conversations.find((c) => peerKey(c) === key);
  if (conv) selectConversation(conv);
}

/* ------------------------------ 痕迹与闪烁 ------------------------------ */

/**
 * 痕迹**不在这里维护**——它完全由会话快照的 `unread_count` + `is_muted` 推导
 * （§3.5 策略矩阵）。这样只有一个数据源：Rust 决定未读，前端只画。
 * 这里只留一个纯 UI 的瞬时状态：折叠条正在闪烁谁。
 */

export function startFlash(peer: PeerRef): void {
  setState('flashKey', keyOf(peer));
}

export function stopFlash(): void {
  setState('flashKey', null);
}

/**
 * 应用 Rust 的提醒决策（§3.5）。
 * 闪不闪、抢不抢折叠条都由 `sched::notify` 判定，前端只负责放动画。
 */
export function applyNotify(intent: NotifyIntent): void {
  const peer: PeerRef = { peer_type: intent.peer_type, peer_id: intent.peer_id };
  const viewing = state.current === keyOf(peer);
  if (intent.flash && !viewing) startFlash(peer);
}

/* ------------------------------ 设置与弹层 ------------------------------ */

export function setSettings(s: SettingsDTO): void {
  setState('settings', s);
}

export function patchSettings(patch: Record<string, unknown>): void {
  setState('settings', (s) => (s === null ? s : ({ ...s, ...patch } as SettingsDTO)));
}

export function setMembers(m: MemberDTO[]): void {
  setState('members', m);
}

export function setViewer(url: string | null): void {
  setState('viewer', url);
}

/** 设置页 / 缓存管理页只在打开时挂载（优化清单 #11：前端按需挂载） */
export function setSheet(sheet: 'settings' | 'cache' | null): void {
  setState('sheet', sheet);
}

export function setQuote(quote: Quote | null): void {
  setState('quote', quote);
}

let toastSeq = 0;

export function showToast(text: string, kind: 'info' | 'error' = 'info'): void {
  const id = (toastSeq += 1);
  setState('toast', { id, text, kind });
  const cur = state.toast;
  setTimeout(() => {
    if (state.toast?.id === cur?.id) setState('toast', null);
  }, 2000);
}

/** 顺手清掉某个会话的消息缓存（清空本地记录后调用） */
export function dropMessages(peer: PeerRef): void {
  setState(
    produce((s) => {
      delete s.messages[keyOf(peer)];
    }),
  );
}

export function peerRefOf(peerType: PeerType, peerId: number): PeerRef {
  return { peer_type: peerType, peer_id: peerId };
}
