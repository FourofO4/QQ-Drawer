/**
 * 折叠条内容合成与优先级（FR-02 / FR-33）。
 *
 * 这是纯函数，Rust 侧 `sched::notify` 里有同一套规则的权威实现——
 * 前端这份**只用于渲染**，不参与判定，避免出现两处不一致的决策。
 */

import type { ConversationDTO } from '../state/types';
import { flatten } from './segments';

export interface BarContent {
  name: string;
  body: string;
}

/** 冒号用全角，与设计稿一致 */
const COLON = '：';

/** 连接断开 / 无会话时折叠条的固定文案（§3.10） */
export const BAR_TEXT = {
  connecting: '连接中…',
  disconnected: '连接已断开 · 点击重试',
  empty: '暂无会话',
} as const;

/**
 * 折叠条文本：`会话名：内容`，群聊额外带上发送者（`群名 · 老王：`）。
 * 超长不在这里截断——交给 CSS `text-overflow: ellipsis`（FR-02：窗口宽度不随内容变化）。
 */
export function composeBar(conv: ConversationDTO): BarContent {
  const body = flatten(conv.last_msg_text ?? '');
  const prefix = barNamePrefix(conv);
  return { name: prefix + COLON, body };
}

/** 会话名前缀：群聊带发送者，私聊只有会话名 */
export function barNamePrefix(conv: ConversationDTO): string {
  const name = conv.name;
  if (conv.peer_type === 0) return name;
  const sender = conv.last_msg_sender?.trim();
  if (!sender || sender === name) return name;
  return `${name} · ${sender}`;
}

/**
 * 折叠条优先级（FR-33）——严格按文档的三档：
 *   ① 有未读的**非静音**会话里最新的一条
 *   ② 没有则未读的静音会话里最新的一条
 *   ③ 都没有未读则显示全局最新的一条
 * 推论：只要存在未读的非静音会话，静音会话永远抢不到折叠条位置。
 */
export function pickBarConversation(
  conversations: readonly ConversationDTO[],
): ConversationDTO | null {
  const pool = conversations.filter((c) => c.last_msg_time !== null);
  if (pool.length === 0) return null;

  const hot = pool.filter((c) => c.unread_count > 0 && !c.is_muted);
  if (hot.length > 0) return latest(hot);

  const anyUnread = pool.filter((c) => c.unread_count > 0);
  if (anyUnread.length > 0) return latest(anyUnread);

  return latest(pool);
}

function latest(list: readonly ConversationDTO[]): ConversationDTO | null {
  let best: ConversationDTO | null = null;
  for (const c of list) {
    const t = c.last_msg_time ?? 0;
    if (best === null || t > (best.last_msg_time ?? 0)) best = c;
  }
  return best;
}

/**
 * 折叠条是否需要带未读痕迹（FR-34：只有痕迹，没有红点、没有数字）。
 * 静音会话在当前会话位时不留痕。
 */
export function barHasMark(conv: ConversationDTO, viewingPeerKey: string | null): boolean {
  if (conv.unread_count <= 0) return false;
  if (conv.is_muted) return false;
  if (viewingPeerKey !== null && peerKey(conv) === viewingPeerKey) return false;
  return true;
}

export function peerKey(p: { peer_type: number; peer_id: number }): string {
  return `${p.peer_type}:${p.peer_id}`;
}

/**
 * 标签 / `···` 项是否需要未读痕迹。
 * @我 与非静音都留痕，但静音会话不闪、不留痕（FR-30）——这里与折叠条口径一致。
 */
export function itemHasMark(conv: ConversationDTO, viewing: boolean, muted: boolean): boolean {
  if (viewing) return false;
  if (muted) return false;
  return conv.unread_count > 0;
}
