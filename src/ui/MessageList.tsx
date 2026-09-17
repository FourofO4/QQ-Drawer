/**
 * 消息列表（FR-19 / FR-20 / NFR-05）。
 *
 * **必须虚拟滚动**：不虚拟化时加载 500 条历史直接把内存拉起来，100 MB 的预算守不住
 * （优化清单 #1）。这里只渲染可视区 ±10 行，行高按需测量。
 *
 * 翻页稳定性的做法：向上插入旧消息后，把「新插入的总高度」加到 scrollTop 上，
 * 视口内容就不会跳动（FR-20）。
 */

import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from 'solid-js';
import * as S from '../state/store';
import * as ipc from '../state/ipc';
import type { MessageRow as Row } from '../core/grouping';
import { MessageRow } from './MessageRow';
import {
  ESTIMATED_ROW_HEIGHT,
  buildOffsets,
  computeRange,
  shouldLoadMore,
} from '../core/vscroll';
import { showMenu } from './ContextMenu';
import { loadOlder } from '../state/events';

const OVERSCAN = 10;

export function MessageList() {
  let scroller: HTMLDivElement | undefined;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewportH, setViewportH] = createSignal(0);
  /** key → 实测高度；没测到的先用估值 */
  const [heights, setHeights] = createSignal<Record<string, number>>({});

  const rows = S.currentRows;

  const offsets = createMemo(() => {
    const h = heights();
    return buildOffsets(rows().map((r) => h[r.key] ?? ESTIMATED_ROW_HEIGHT));
  });

  const range = createMemo<Row[]>(() => {
    const r = computeRange(scrollTop(), viewportH(), offsets(), OVERSCAN);
    return rows().slice(r.start, r.end);
  });

  const pads = createMemo(() => computeRange(scrollTop(), viewportH(), offsets(), OVERSCAN));

  /** 是否贴底——贴底时来新消息要自动跟到最新 */
  const [pinned, setPinned] = createSignal(true);

  /** 行高测量：用 ResizeObserver，行内图片加载完高度变化也能跟上 */
  let observer: ResizeObserver | undefined;
  const measure = (el: HTMLDivElement, key: string) => {
    observer?.observe(el);
    el.dataset.key = key;
  };

  onMount(() => {
    observer = new ResizeObserver((entries) => {
      let changed = false;
      const next = { ...heights() };
      for (const e of entries) {
        const el = e.target as HTMLElement;
        const key = el.dataset.key;
        if (key === undefined) continue;
        const h = el.offsetHeight;
        if (next[key] !== h) {
          next[key] = h;
          changed = true;
        }
      }
      if (changed) setHeights(next);
    });

    const onResize = () => {
      if (scroller) setViewportH(scroller.clientHeight);
    };
    onResize();
    window.addEventListener('resize', onResize);
    onCleanup(() => {
      window.removeEventListener('resize', onResize);
      observer?.disconnect();
    });
  });

  const onScroll = () => {
    const el = scroller;
    if (!el) return;
    setScrollTop(el.scrollTop);
    const bottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 40;
    setPinned(bottom);
    if (shouldLoadMore(el.scrollTop)) void paginateUp();
  };

  /** 向上翻页：记录插入前的高度，插入后把滚动位置顶回原处 */
  let loading = false;
  const paginateUp = async () => {
    const el = scroller;
    if (el === undefined || loading) return;
    const before = el.scrollHeight;
    const beforeTop = el.scrollTop;
    loading = true;
    await loadOlder();
    loading = false;
    // 等一帧让 DOM 落地，再把差值补回 scrollTop
    requestAnimationFrame(() => {
      const delta = el.scrollHeight - before;
      if (delta > 0) el.scrollTop = beforeTop + delta;
    });
  };

  /** 切会话时跳到底部；贴底时来新消息也跟到底部 */
  createEffect(() => {
    const key = S.state.current;
    const n = rows().length;
    void n;
    const el = scroller;
    if (!el) return;
    if (key === null) return;
    queueMicrotask(() => {
      if (pinned() || el.scrollTop === 0) el.scrollTop = el.scrollHeight;
    });
  });

  const jumpTo = (messageId: string) => {
    const el = scroller?.querySelector<HTMLElement>(`[data-mid="${messageId}"]`);
    if (el) {
      el.scrollIntoView({ block: 'center' });
      el.animate?.(
        [
          { outline: '1px solid rgba(239,159,39,.9)' },
          { outline: '1px solid rgba(239,159,39,0)' },
        ],
        { duration: 900 },
      );
      return;
    }
    S.showToast('这条消息不在当前加载的范围内');
  };

  const onBlankMenu = (e: MouseEvent) => {
    showMenu(e, [
      { label: '标记全部已读', action: () => void ipc.markAllRead() },
      {
        label: '刷新历史',
        action: () => {
          const conv = S.currentConversation();
          if (conv) void ipc.refreshConversations();
        },
      },
      { label: '设置', action: () => S.setSheet('settings') },
    ]);
  };

  return (
    <div class="msgs" ref={scroller} onScroll={onScroll} onContextMenu={onBlankMenu}>
      <Show when={S.currentAtTop()}>
        <div class="top-hint">以上是全部</div>
      </Show>
      <Show when={S.state.historyLoading}>
        <div class="loading-hint">正在加载更早的消息…</div>
      </Show>

      <div class="vr-pad" style={{ height: `${pads().padTop}px` }} />

      <For each={range()}>
        {(row) => (
          <div class="row-wrap" ref={(el) => measure(el, row.key)} data-mid={row.msg.message_id}>
            <MessageRow row={row} selfId={S.state.selfId} onJump={jumpTo} />
          </div>
        )}
      </For>

      <div class="vr-pad" style={{ height: `${pads().padBottom}px` }} />

      <Show when={rows().length === 0}>
        <div class="empty-hint">
          {S.state.conn === 'connected' ? '还没有消息' : '未连接到 NapCat，点击重试'}
        </div>
      </Show>
    </div>
  );
}
