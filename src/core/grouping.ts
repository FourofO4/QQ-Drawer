/**
 * 消息列表到行模型的转换（FR-17 / FR-19）。
 *
 * 纯函数：输入「按时间升序的消息数组」，输出带分隔、带合并标记的行数组。
 * 虚拟滚动直接消费这个结果，所以它必须便宜——只做一次线性扫描。
 */

import type { MessageDTO } from '../state/types';
import { SEPARATOR_GAP_MS, separatorFor, type Separator } from './time';

/** 连续同一人消息的缩进 = 头像 26 + 间距 8（§3.3） */
export const CONTINUATION_INDENT = 34;

export interface MessageRow {
  msg: MessageDTO;
  /** 该行上方要插入的分隔，null 表示不插 */
  separator: Separator;
  /** 同一人连续发言的后续条目：缩进、不显示头像与名字 */
  cont: boolean;
  /** 是否显示发送者头像与名字。群聊且非连续续行、且不是自己发的才显示 */
  showWho: boolean;
  /** 列表内的稳定键：虚拟滚动靠它做锚点记忆 */
  key: string;
}

/**
 * 构建行模型。
 * @param messages 同一会话的消息，**必须按 ts 升序**
 * @param now 用于「今天/昨天」判定，注入以便测试
 */
export function buildRows(
  messages: readonly MessageDTO[],
  now: number = Date.now(),
): MessageRow[] {
  const rows: MessageRow[] = [];
  let prev: MessageDTO | null = null;

  for (const msg of messages) {
    const separator = separatorFor(prev === null ? null : prev.ts, msg.ts, now);
    const cont = isContinuation(prev, msg, separator);
    // 私聊不标名（会话名即发送者）；自己发的一律不标名，右对齐
    const showWho = msg.peer_type === 1 && !msg.is_self && !cont;

    rows.push({
      msg,
      separator,
      cont,
      showWho,
      key: msg.message_id,
    });
    prev = msg;
  }
  return rows;
}

function isContinuation(
  prev: MessageDTO | null,
  cur: MessageDTO,
  separator: Separator,
): boolean {
  if (prev === null || separator !== null) return false;
  if (prev.sender_id !== cur.sender_id) return false;
  if (prev.is_self !== cur.is_self) return false;
  if (cur.ts - prev.ts > SEPARATOR_GAP_MS) return false;
  return true;
}

/**
 * 取出消息列表里需要向远端补历史的边界。
 * 返回 null 表示本地还有更早的消息，不需要请求远端（§4.6 历史分页）。
 */
export function oldestSeq(rows: readonly MessageRow[]): number | null {
  const first = rows[0];
  return first ? first.msg.seq : null;
}
