/**
 * invoke / listen 的唯一出口（§4.2）。
 *
 * 规则：**前端不允许在别处直接 import @tauri-apps/api**。
 * 所有命令名与事件名都在这里集中声明，改 IPC 只改这一个文件。
 *
 * 开发期彩蛋：不在 Tauri 里运行时（直接 `npm run dev` 用浏览器打开）会自动挂上
 * `src/dev/mock.ts` 的假后端，方便脱离 NapCat 调界面。生产构建里这段永远不激活。
 */

import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type {
  CacheOverviewDTO,
  ConnectionState,
  ConversationDTO,
  MemberDTO,
  MessageDTO,
  NotifyIntent,
  PeerType,
  SettingsDTO,
} from './types';
import type { Part } from '../core/composer-model';

/** 命令名 与 Rust `cmd.rs` 里的 #[tauri::command] 必须逐字一致 */
export const CMD = {
  connStatus: 'conn_status',
  retryConnect: 'retry_connect',
  conversations: 'list_conversations',
  loadMessages: 'load_messages',
  sendMessage: 'send_message',
  retrySend: 'retry_send',
  markRead: 'mark_read',
  markAllRead: 'mark_all_read',
  setMute: 'set_mute',
  deleteMessage: 'delete_message',
  recallMessage: 'recall_message',
  addTab: 'add_tab',
  removeTab: 'remove_tab',
  reorderTabs: 'reorder_tabs',
  refreshConversations: 'refresh_conversations',
  listMembers: 'list_members',
  cacheOverview: 'cache_overview',
  clearCache: 'clear_cache',
  getSettings: 'get_settings',
  setSetting: 'set_setting',
  savePastedImage: 'save_pasted_image',
  setViewing: 'set_viewing',
  expand: 'expand_window',
  collapse: 'collapse_window',
  exit: 'exit_app',
} as const;

/** 事件名 与 Rust 侧 emit 的字符串必须逐字一致 */
export const EV = {
  connState: 'conn_state',
  conversations: 'conversations',
  msgAdded: 'msg_added',
  msgUpdated: 'msg_updated',
  msgRemoved: 'msg_removed',
  notify: 'notify',
  historyPage: 'history_page',
  autoCollapse: 'auto_collapse',
  toggle: 'toggle_panel',
  openSheet: 'open_sheet',
  toast: 'toast',
} as const;

export interface HistoryPage {
  peer_type: PeerType;
  peer_id: number;
  messages: MessageDTO[];
  /** 远端也没有更早的了 */
  at_top: boolean;
}

export interface ToastPayload {
  text: string;
  kind?: 'info' | 'error';
}

/**
 * 新消息事件：**顺带带上该会话的最新快照**。
 * 这样前端不必为了刷新未读数去重拉整个会话列表——消息到达 → 界面更新 ≤ 150 ms（NFR-04）。
 */
export interface MessageAddedPayload {
  message: MessageDTO;
  /**
   * 该会话的最新快照。
   *
   * 可能为 `null`：Rust 侧取快照失败（会话行恰好不存在）时不会为了一个事件回滚消息写入，
   * 所以前端必须容忍缺失——不能假设它一定有。
   */
  conversation: ConversationDTO | null;
}

/** 是否跑在 Tauri WebView2 里 */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/**
 * 是否启用开发期假后端（`src/dev/mock.ts`）。
 *
 * 三个条件同时满足才启用：开发模式、有 DOM、不在测试里。
 * 生产构建下 `import.meta.env.DEV` 是编译期常量 false，整个分支会被 tree-shake 掉——
 * 实测 dist 产物里不含 mock 的任何代码。
 */
const USE_DEV_MOCK =
  import.meta.env.DEV && import.meta.env.MODE !== 'test' && typeof window !== 'undefined';

let mockActive = false;
let mockReady: Promise<void> | null = null;

async function ensureBackend(): Promise<void> {
  if (isTauri() || !USE_DEV_MOCK) return;
  if (mockReady === null) {
    mockReady = import('./../dev/mock').then((m) => m.installMockBackend());
    mockActive = true;
  }
  await mockReady;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  await ensureBackend();
  if (mockActive && !isTauri()) {
    const { mockInvoke } = await import('./../dev/mock');
    return mockInvoke<T>(cmd, args);
  }
  return invoke<T>(cmd, args);
}

async function on<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
  await ensureBackend();
  if (mockActive && !isTauri()) {
    const { mockListen } = await import('./../dev/mock');
    return mockListen<T>(event, handler);
  }
  return listen<T>(event, (e) => handler(e.payload));
}

/* ------------------------------ 连接 ------------------------------ */

/** 连接状态 + 自己的 QQ 号（渲染 @我 判定与"不标名"规则时需要） */
export interface ConnInfo {
  state: ConnectionState;
  self_id: number | null;
}

export const connStatus = () => call<ConnInfo>(CMD.connStatus);
export const retryConnect = () => call<void>(CMD.retryConnect);

/* ------------------------------ 会话 ------------------------------ */

export const listConversations = () => call<ConversationDTO[]>(CMD.conversations);
export const refreshConversations = (count = 20) =>
  call<void>(CMD.refreshConversations, { count });

/* ------------------------------ 消息 ------------------------------ */

export const loadMessages = (
  peerType: PeerType,
  peerId: number,
  beforeSeq: number | null,
  limit = 30,
) => call<MessageDTO[]>(CMD.loadMessages, { peerType, peerId, beforeSeq, limit });

export const sendMessage = (peerType: PeerType, peerId: number, parts: Part[]) =>
  call<{ message_id: string }>(CMD.sendMessage, { peerType, peerId, parts });

export const retrySend = (messageId: string) =>
  call<{ message_id: string }>(CMD.retrySend, { messageId });

export const markRead = (peerType: PeerType, peerId: number) =>
  call<void>(CMD.markRead, { peerType, peerId });

export const markAllRead = () => call<void>(CMD.markAllRead);

/** 墓碑式手动删除：只写本地墓碑表，不上报 QQ（FR-45） */
export const deleteMessage = (messageId: string) =>
  call<void>(CMD.deleteMessage, { messageId });

/** 撤回自己发出的消息——会让消息在对方那边也消失，与本地删除完全不是一件事（§4.6） */
export const recallMessage = (messageId: string) =>
  call<void>(CMD.recallMessage, { messageId });

/* ------------------------------ 标签 ------------------------------ */

export const addTab = (peerType: PeerType, peerId: number) =>
  call<void>(CMD.addTab, { peerType, peerId });
export const removeTab = (peerType: PeerType, peerId: number) =>
  call<void>(CMD.removeTab, { peerType, peerId });
export const reorderTabs = (order: string[]) => call<void>(CMD.reorderTabs, { order });

/* ------------------------------ 静音 ------------------------------ */

/** 只写本地库，不调用任何 NapCat 接口（FR-37） */
export const setMute = (peerType: PeerType, peerId: number, muted: boolean) =>
  call<void>(CMD.setMute, { peerType, peerId, muted });

/* ------------------------------ 成员 ------------------------------ */

export const listMembers = (groupId: number, query = '') =>
  call<MemberDTO[]>(CMD.listMembers, { groupId, query });

/* ------------------------------ 缓存 ------------------------------ */

export const cacheOverview = () => call<CacheOverviewDTO>(CMD.cacheOverview);
export const clearCache = (
  scope: 'peer' | 'age' | 'all',
  target?: { peerType: PeerType; peerId: number; days: number },
) => call<void>(CMD.clearCache, { scope, ...(target ?? {}) });

/**
 * 本地图片路径 → WebView2 可直接加载的地址。
 *
 * 走的是 Rust 侧自己注册的 `media` 协议（`http://media.localhost/<路径>`），
 * 不是 Tauri 内置的 asset 协议 —— 内置那个要在 Cargo 里开 `protocol-asset`
 * 并把目录写进 `tauri.conf.json` 的 allowlist，等于把整个 media 目录暴露给
 * 页面；自定义协议由 `media::serve` 逐请求校验，能挡住路径穿越。
 */
export const imageUrl = (relPath: string): string =>
  isTauri() ? convertFileSrc(relPath, 'media') : relPath;

/* ------------------------------ 设置 ------------------------------ */

export const getSettings = () => call<SettingsDTO>(CMD.getSettings);
export const setSetting = (key: string, value: unknown) =>
  call<void>(CMD.setSetting, { key, value });

/**
 * 粘贴的图片先落盘换一个本地绝对路径（FR-26）。
 * 发送时只把路径交给 Rust —— 同机运行，WebSocket 只传几十字节，
 * 几 MB 的截图不会撑爆连接，也避免 base64 的内存尖峰（§4.6）。
 */
export const savePastedImage = (dataUrl: string) =>
  call<{ sha256: string; path: string }>(CMD.savePastedImage, { dataUrl });

/* ------------------------------ 窗口 ------------------------------ */

/** 展开/收起：窗口几何由 Rust 掌握，前端只发意图（§4.1 窗口与内容解耦） */
export const expand = () => call<void>(CMD.expand);
export const collapse = () => call<void>(CMD.collapse);
export const exitApp = () => call<void>(CMD.exit);

/**
 * 告诉 Rust「我正在看哪个会话」。
 *
 * 提醒决策完全依赖它：没有这一步，盯着某个会话时每来一条消息都会闪（FR-31），
 * 而且重连补齐也找不到"当前会话"。传 null 表示没有在看的会话。
 */
export const setViewing = (peerType: PeerType | null, peerId: number | null) =>
  call<void>(CMD.setViewing, { peerType, peerId });

/** 交给系统做拖动——只有位移超过 3px 才调用，避免"一拖就展开"（踩坑 #7） */
export const startDragging = async (): Promise<void> => {
  if (!isTauri()) return;
  await getCurrentWindow().startDragging();
};


/* ------------------------------ 事件订阅 ------------------------------ */

export const onConnState = (h: (s: ConnInfo) => void) => on(EV.connState, h);
export const onConversations = (h: (list: ConversationDTO[]) => void) =>
  on(EV.conversations, h);
export const onMessageAdded = (h: (p: MessageAddedPayload) => void) =>
  on(EV.msgAdded, h);
export const onMessageUpdated = (h: (m: MessageDTO) => void) => on(EV.msgUpdated, h);
/** 乐观条目被真实消息替换时，Rust 会先让本地临时条目消失（FR-24） */
export const onMessageRemoved = (h: (messageId: string) => void) =>
  on<string>(EV.msgRemoved, h);
export const onNotify = (h: (i: NotifyIntent) => void) => on(EV.notify, h);
export const onHistoryPage = (h: (p: HistoryPage) => void) => on(EV.historyPage, h);
export const onAutoCollapse = (h: () => void) => on<void>(EV.autoCollapse, () => h());
export const onTogglePanel = (h: () => void) => on<void>(EV.toggle, () => h());
/** 托盘菜单请求打开浮层：载荷是 `'settings' | 'cache'` */
export const onOpenSheet = (h: (sheet: string) => void) => on<string>(EV.openSheet, h);
export const onToast = (h: (t: ToastPayload) => void) => on(EV.toast, h);
