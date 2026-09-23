/**
 * 消息列表（FR-19 / FR-20 / NFR-05）。
 *
 * **必须虚拟滚动**：不虚拟化时加载 500 条历史直接把内存拉起来，100 MB 的预算守不住
 * （优化清单 #1）。这里只渲染可视区 ±10 行，行高按需测量。
 *
 * ## 视口位置由锚点决定，不由 scrollTop 决定
 *
 * `scrollTop` 是绝对像素值，但它所在的内容坐标系 `offsets` 会变：行高从估值换成
 * 实测、向上翻页插入历史，都会改写它。坐标系一变，同一个 `scrollTop` 就指向别的内容
 * 了 —— 用户看到的就是"滚着滚着消息自己往回退"（bug 3）。
 *
 * 所以每次坐标系换代（测量回填 / 翻页插入），都先用**旧**坐标系取出锚点
 * （哪一行 + 行内偏移），再用**新**坐标系反算 `scrollTop`。见 `core/vscroll.ts`
 * 的 `anchorKeyAt` / `topForKey`。
 */

import { For, Show, createEffect, createMemo, createSignal, onCleanup } from 'solid-js';
import * as S from '../state/store';
import * as ipc from '../state/ipc';
import type { MessageRow as Row } from '../core/grouping';
import { MessageRow } from './MessageRow';
import {
  anchorKeyAt,
  buildOffsets,
  computeRange,
  estimateRowHeight,
  isAppendOnly,
  shouldLoadMore,
  topForKey,
} from '../core/vscroll';
import { showMenu } from './ContextMenu';
import { loadOlder } from '../state/events';

const OVERSCAN = 10;
/** 距底部多少像素以内算「贴底」。贴底时才自动跟随新消息 */
const PINNED_SLACK = 40;
/** 观察目标攒到这么多就先清一遍已滚出渲染区的，别让 ResizeObserver 的列表无限膨胀 */
const OBSERVE_PRUNE_AT = 200;

export function MessageList() {
  let scroller: HTMLDivElement | undefined;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewportH, setViewportH] = createSignal(0);
  /** key → 实测高度；没测到的先用 `estimate()` */
  const [heights, setHeights] = createSignal<Record<string, number>>({});
  /** 是否贴底——只有贴底时才跟随新消息 */
  const [pinned, setPinned] = createSignal(true);

  const rows = S.currentRows;
  const rowKeys = createMemo(() => rows().map((r) => r.key));

  /**
   * 未测行的估值：取已测行高的中位数，而不是固定的 44。
   * 固定估值与真实行高差得越多，渲染窗口每滑动一行时的换算就越不平，
   * `scrollHeight` 抖得越厉害 —— 滚动条抽风就是这么来的。
   */
  const estimate = createMemo(() => estimateRowHeight(Object.values(heights())));

  const offsets = createMemo(() => {
    const h = heights();
    const est = estimate();
    return buildOffsets(rows().map((r) => h[r.key] ?? est));
  });

  const win = createMemo(() => computeRange(scrollTop(), viewportH(), offsets(), OVERSCAN));
  const range = createMemo<Row[]>(() => {
    const w = win();
    return rows().slice(w.start, w.end);
  });

  /** 写滚动位置——必须同时更新信号，否则 range 会拿旧位置多渲染一帧，看着就是抖 */
  const writeScrollTop = (el: HTMLDivElement, top: number) => {
    if (el.scrollTop !== top) el.scrollTop = top;
    // 浏览器会把 scrollTop 夹到 maxScrollTop，读回来的才是真实值
    setScrollTop(el.scrollTop);
  };

  /* ------------------------------ 行高测量 ------------------------------ */

  const observed = new Set<HTMLElement>();

  /** 清掉已经滚出渲染区的观察目标（ResizeObserver 不会自己松手，不清就是内存泄漏） */
  const prune = () => {
    for (const el of observed) {
      if (!el.isConnected) {
        observer.unobserve(el);
        observed.delete(el);
      }
    }
  };

  const observeRow = (el: HTMLDivElement, key: string) => {
    el.dataset.key = key;
    if (observed.size > OBSERVE_PRUNE_AT) prune();
    observer.observe(el);
    observed.add(el);
  };

  /**
   * 提交一批测量结果，并把「高度变了」造成的视口偏移补回去。
   * 这是 bug 3 的核心修法：测量会让 offsets 换代，不补偿视口就会跳。
   * 贴底时改为跟到底部 —— 那时用户要的就是最新一条。
   */
  const commitHeights = (next: Record<string, number>) => {
    const el = scroller;
    if (!el) return;
    const keys = rowKeys();
    const anchor = anchorKeyAt(offsets(), keys, el.scrollTop);
    const stick = pinned();

    const est = estimateRowHeight(Object.values(next));
    const nextOffsets = buildOffsets(rows().map((r) => next[r.key] ?? est));
    setHeights(next);

    // 等 Solid 把新的占位高度写进 DOM 再纠正滚动位置。同一个任务内完成，用户看不到中间态。
    queueMicrotask(() => {
      if (stick) {
        writeScrollTop(el, el.scrollHeight);
        return;
      }
      if (anchor === null) return;
      const top = topForKey(nextOffsets, keys, anchor.key, anchor.inner);
      if (top !== null) writeScrollTop(el, top);
    });
  };

  const handleResize = (entries: ResizeObserverEntry[]) => {
    const next = { ...heights() };
    let changed = false;
    for (const e of entries) {
      const el = e.target as HTMLElement;
      if (el === scroller) {
        // 容器自己变了（面板展开 / 收起 / 窗口缩放）→ 视口高度跟着变
        setViewportH(el.clientHeight);
        continue;
      }
      if (!el.isConnected) {
        observer.unobserve(el);
        observed.delete(el);
        continue;
      }
      const key = el.dataset.key;
      if (key === undefined || key === '') continue;
      const h = el.offsetHeight;
      if (next[key] !== h) {
        next[key] = h;
        changed = true;
      }
    }
    if (changed) commitHeights(next);
  };

  // 观察者必须在这里就建好，不能等 `onMount`：JSX 的 `ref` 回调比 `onMount` 先跑，
  // 等 onMount 再建的话**首屏渲染出来的行一个都不会被观察**，高度一直停在估值上，
  // 直到用户开始滚动才补测 —— 那就是「一滚就大幅抖动」。
  const observer = new ResizeObserver(handleResize);
  onCleanup(() => {
    observer.disconnect();
    observed.clear();
  });

  /* ------------------------------ 滚动与翻页 ------------------------------ */

  const onScroll = () => {
    const el = scroller;
    if (!el) return;
    setScrollTop(el.scrollTop);
    setPinned(el.scrollTop + el.clientHeight >= el.scrollHeight - PINNED_SLACK);
    if (shouldLoadMore(el.scrollTop)) void paginateUp();
  };

  let loading = false;
  /**
   * 向上翻页（FR-20）。
   *
   * 锚点必须在 `loadOlder()` **之前**取：加载完 rows 已经变了，那时再取就对不上。
   * 旧实现拿 `scrollHeight` 的差值去补 scrollTop —— 那个差值里混着行高测量、图片
   * 撑开等一堆无关变化，而且只等一帧就补，补出来的位置是错的。
   */
  const paginateUp = async () => {
    const el = scroller;
    if (el === undefined || loading) return;
    // 旧坐标系要整份存下来。等待 IPC 的这几十~几百毫秒里用户还会继续滚，
    // 那几下滚动都是按**旧**坐标系记的数值，直接套到新坐标系上就会错位。
    const beforeOffsets = offsets();
    const beforeKeys = rowKeys();
    loading = true;
    // 翻页意味着用户在看历史：明确退出贴底，否则紧随其后的行高测量会把视口拽回底部
    setPinned(false);
    try {
      await loadOlder();
    } finally {
      loading = false;
    }
    // 把「用户此刻的滚动位置」当作旧坐标系的坐标，映射到新坐标系。
    // 用户没滚 → 视口原地不动；用户滚过 → 尊重他滚到的地方。
    // 两者都覆盖，才不会出现「滚了好几下却卡在中间」（bug 3）。
    const anchor = anchorKeyAt(beforeOffsets, beforeKeys, el.scrollTop);
    if (anchor === null) return;
    const top = topForKey(offsets(), rowKeys(), anchor.key, anchor.inner);
    if (top !== null) writeScrollTop(el, top);
  };

  /* ------------------- 切会话跳底 / 追加时跟随底部 ------------------- */

  let lastPeer: string | null = null;
  let lastKeys: string[] = [];
  let jumpPending = false;

  createEffect(() => {
    const peer = S.state.current;
    const keys = rowKeys();
    const el = scroller;
    if (!el) return;

    if (peer !== lastPeer) {
      lastPeer = peer;
      lastKeys = [];
      jumpPending = true;
      setPinned(true); // 换会话默认从最新看起
    }
    if (keys.length === 0) return;

    if (jumpPending) {
      jumpPending = false;
      lastKeys = keys;
      queueMicrotask(() => writeScrollTop(el, el.scrollHeight));
      return;
    }

    const prev = lastKeys;
    lastKeys = keys;
    // 只有「向下追加」才跟随。向上翻页插入历史时首行会变，那时跟到底部就成了
    // 「往上翻一页被弹回底部」的鬼打墙 —— 也是 bug 3 里"滚轮复位"的直接原因。
    if (!isAppendOnly(prev, keys)) return;
    if (!pinned()) return;
    queueMicrotask(() => writeScrollTop(el, el.scrollHeight));
  });

  const jumpTo = (messageId: string) => {
    const el = scroller?.querySelector<HTMLElement>(`[data-mid="${messageId}"]`);
    if (el && scroller) {
      el.scrollIntoView({ block: 'center' });
      setScrollTop(scroller.scrollTop);
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
    <div
      class="msgs"
      ref={(el) => {
        scroller = el;
        observer.observe(el); // 容器尺寸变化（面板展开 / 收起 / 窗口缩放）也要跟上
        setViewportH(el.clientHeight);
      }}
      onScroll={onScroll}
      onContextMenu={onBlankMenu}
    >
      <Show when={S.currentAtTop()}>
        <div class="top-hint">以上是全部</div>
      </Show>
      <Show when={S.state.historyLoading}>
        <div class="loading-hint">正在加载更早的消息…</div>
      </Show>

      <div class="vr-pad" style={{ height: `${win().padTop}px` }} />

      <For each={range()}>
        {(row) => (
          <div
            class="row-wrap"
            ref={(el) => observeRow(el, row.key)}
            data-mid={row.msg.message_id}
          >
            <MessageRow row={row} selfId={S.state.selfId} onJump={jumpTo} />
          </div>
        )}
      </For>

      <div class="vr-pad" style={{ height: `${win().padBottom}px` }} />

      <Show when={rows().length === 0}>
        <div class="empty-hint">
          {S.state.conn === 'connected' ? '还没有消息' : '未连接到 NapCat，点击重试'}
        </div>
      </Show>
    </div>
  );
}
