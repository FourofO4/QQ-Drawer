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
    rows.push({ ...deriveRow(prev, msg, now), msg, key: msg.message_id });
    prev = msg;
  }
  return rows;
}

/**
 * 分隔是不是同一份。
 *
 * ⚠️ 不能直接 `===`：`separatorFor` 每次返回**新对象**，引用永远不等，
 * 判定会恒为 false —— 缓存静悄悄地失效，行为退回"整窗重建"，而且看不出错。
 */
function sameSeparator(a: Separator | null, b: Separator | null): boolean {
  if (a === b) return true;
  if (a === null || b === null) return false;
  return a.kind === b.kind && a.text === b.text;
}

/** 一行的全部派生字段（不含 msg 与 key 本身）——`buildRows` 与行缓存共用同一份判定 */
function deriveRow(
  prev: MessageDTO | null,
  msg: MessageDTO,
  now: number,
): Pick<MessageRow, 'separator' | 'cont' | 'showWho'> {
  const separator = separatorFor(prev === null ? null : prev.ts, msg.ts, now);
  const cont = isContinuation(prev, msg, separator);
  // 私聊不标名（会话名即发送者）；自己发的一律不标名，右对齐
  const showWho = msg.peer_type === 1 && !msg.is_self && !cont;
  return { separator, cont, showWho };
}

/**
 * 行对象缓存：内容没变的行**复用同一个对象引用**。
 *
 * 为什么这件事是性能的关键：`<For>` 按**引用**做 diff。如果每次都产出全新的行对象，
 * 引用全变 = 整个渲染窗口（±10 行，约 30 个）的 DOM 全部销毁重建。一次重建就是
 * 30 个 `ref` 回调 → 30 次 `ResizeObserver` 首测 → 一轮行高回填 → 回填改写坐标系 →
 * 又触发一轮渲染。快速滚动时这个环会自激，主线程被吃满，滚轮就"不动了"。
 *
 * 有缓存之后，新来一条消息只新建 1 个行对象、只创建 1 个 DOM 节点，其余全部复用。
 */
export interface RowCache {
  build(messages: readonly MessageDTO[], now?: number): MessageRow[];
  /** 清空缓存（换账号等整表重置的场合） */
  clear(): void;
  /** 当前缓存的行数——测试与排障用 */
  size(): number;
}

export function createRowCache(): RowCache {
  const cache = new Map<string, MessageRow>();

  return {
    build(messages, now = Date.now()) {
      const out: MessageRow[] = [];
      const live = new Set<string>();
      let prev: MessageDTO | null = null;

      for (const msg of messages) {
        const key = msg.message_id;
        live.add(key);

        const hit = cache.get(key);
        const derived = deriveRow(prev, msg, now);
        if (
          hit !== undefined &&
          hit.msg === msg &&
          hit.cont === derived.cont &&
          hit.showWho === derived.showWho &&
          sameSeparator(hit.separator, derived.separator)
        ) {
          out.push(hit);
        } else {
          const row: MessageRow = { ...derived, msg, key };
          cache.set(key, row);
          out.push(row);
        }
        prev = msg;
      }

      // 不在当前列表里的（切会话、删消息、换账号）连同缓存一起丢掉，避免无界增长。
      // 只在确实有多余项时遍历，省掉每帧一次的全量扫描。
      if (cache.size > live.size) {
        for (const k of cache.keys()) if (!live.has(k)) cache.delete(k);
      }
      return out;
    },

    clear() {
      cache.clear();
    },

    size() {
      return cache.size;
    },
  };
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
