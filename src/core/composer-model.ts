/**
 * 输入框的受控模型（FR-28）。
 *
 * 这是 MVP 里最重的一块交互：输入框不是普通 textarea，而是「文本片段 + 令牌」交错的
 * 受控编辑层。模型是一维数组，令牌是**原子项**——不可被字符级编辑破坏，只能整体删除。
 *
 * 这里只放纯数据操作，DOM 与光标逻辑在 ui/Composer.tsx。
 * 发送时把这个数组交给 Rust，由 `ob::segments::build_outgoing` 转成 OneBot 消息段
 * ——协议序列化只出现在 ob 层（NFR-13）。
 */

import type { PeerType } from '../state/types';

export type Part =
  | { kind: 'text'; text: string }
  | { kind: 'at'; qq: number; name: string }
  | { kind: 'image'; sha256: string; path: string }
  /** 引用回复：不参与正文，只作为发送时的 reply 段 */
  | { kind: 'reply'; id: string };

export type SendBlockReason = 'empty' | 'at_in_private';

export interface SendCheck {
  ok: boolean;
  reason?: SendBlockReason;
}

/** 令牌的展示文案（纯文字，不做缩略图预览 —— FR-26 / §3.3 输入区） */
export const AT_PREFIX = '@';
export const IMAGE_LABEL = '[图片]';

export function partLabel(part: Part): string {
  switch (part.kind) {
    case 'text':
      return part.text;
    case 'at':
      return `${AT_PREFIX}${part.name}`;
    case 'image':
      return IMAGE_LABEL;
    case 'reply':
      // 引用块单独渲染，不占正文
      return '';
  }
}

/**
 * 整理模型：丢掉空文本片段，把相邻文本片段合并。
 * 每次改动后都应该跑一遍，否则会出现大量零碎片段，序列化结果里全是空 text 段。
 */
export function normalize(parts: readonly Part[]): Part[] {
  const out: Part[] = [];
  for (const p of parts) {
    if (p.kind === 'text') {
      if (p.text === '') continue;
      const last = out[out.length - 1];
      if (last && last.kind === 'text') {
        out[out.length - 1] = { kind: 'text', text: last.text + p.text };
      } else {
        out.push({ kind: 'text', text: p.text });
      }
    } else {
      out.push(p);
    }
  }
  return out;
}

/** 光标定位用：模型里第 index 项之前的位置插入 */
export function insertAt(parts: readonly Part[], index: number, part: Part): Part[] {
  const next = [...parts];
  next.splice(clampIndex(index, next.length), 0, part);
  return normalize(next);
}

export function insertText(parts: readonly Part[], index: number, text: string): Part[] {
  if (text === '') return [...parts];
  return insertAt(parts, index, { kind: 'text', text });
}

export function appendText(parts: readonly Part[], text: string): Part[] {
  return insertText(parts, parts.length, text);
}

/** 令牌整体删除（FR-28：令牌不可被字符级编辑破坏） */
export function removePart(parts: readonly Part[], index: number): Part[] {
  const next = [...parts];
  next.splice(clampIndex(index, next.length), 1);
  return normalize(next);
}

export function clearParts(): Part[] {
  return [];
}

export function plainText(parts: readonly Part[]): string {
  return parts.map(partLabel).join('');
}

/**
 * 去掉纯空白后是否真的没内容了。
 * 引用段（reply）不算内容——只挂了一条引用、正文空白时不允许发送。
 */
export function isEmpty(parts: readonly Part[]): boolean {
  return parts.every((p) => p.kind === 'reply' || (p.kind === 'text' && p.text.trim() === ''));
}

export function countImages(parts: readonly Part[]): number {
  return parts.filter((p) => p.kind === 'image').length;
}

export function hasAt(parts: readonly Part[]): boolean {
  return parts.some((p) => p.kind === 'at');
}

export function quoteOf(parts: readonly Part[]): string | null {
  const r = parts.find((p) => p.kind === 'reply');
  return r && r.kind === 'reply' ? r.id : null;
}

/** 已经插入过同一个人就不再重复插入 */
export function hasAtUser(parts: readonly Part[], qq: number): boolean {
  return parts.some((p) => p.kind === 'at' && p.qq === qq);
}

/**
 * 发送前校验（§4.7）：
 * · 空内容不发送
 * · 纯空白不发送
 * · 仅有令牌无文本时按「图片消息」正常发送 —— 即令牌存在就放行
 * · 私聊不允许 at 令牌
 */
export function canSend(parts: readonly Part[], peerType: PeerType): SendCheck {
  if (parts.length === 0) return { ok: false, reason: 'empty' };
  if (isEmpty(parts)) return { ok: false, reason: 'empty' };
  if (peerType === 0 && hasAt(parts)) return { ok: false, reason: 'at_in_private' };
  return { ok: true };
}

export function blockReasonText(reason: SendBlockReason): string {
  switch (reason) {
    case 'empty':
      return '没有可发送的内容';
    case 'at_in_private':
      return '私聊里不能 @ 人';
  }
}

function clampIndex(i: number, len: number): number {
  if (i < 0) return 0;
  if (i > len) return len;
  return i;
}

/**
 * @ 弹层的搜索过滤（FR-27）。
 * **只做文本过滤、不显示头像**——几百个头像的网络与解码开销纯浪费（优化清单 #6）。
 */
export function filterMembers<T extends { display_name: string; user_id: number }>(
  members: readonly T[],
  query: string,
  limit = 8,
): T[] {
  const q = query.trim().toLowerCase();
  const matched = q === ''
    ? [...members]
    : members.filter(
        (m) => m.display_name.toLowerCase().includes(q) || String(m.user_id).includes(q),
      );
  return matched.slice(0, limit);
}
