/**
 * 虚拟滚动数学（NFR-05 / 优化清单 #1）。
 *
 * 只渲染可视区 ±overscan 行；累计高度表定位；翻页插入历史时靠 anchorShift 保持视口稳定。
 * 这里刻意不碰 DOM——DOM 测量在 MessageList 里做，本模块只负责算下标。
 */

export interface Range {
  /** 起始行下标（含） */
  start: number;
  /** 结束行下标（不含） */
  end: number;
  /** 上方的占位高度 */
  padTop: number;
  /** 下方的占位高度 */
  padBottom: number;
}

/** 行高未知时的估值，第一帧用它兜底，测量后逐行替换 */
export const ESTIMATED_ROW_HEIGHT = 44;

/**
 * 累计偏移表：offsets[i] = 第 i 行的顶部坐标，长度 = heights.length + 1。
 * 末尾元素即总高度。
 */
export function buildOffsets(heights: readonly number[]): number[] {
  const offsets = new Array<number>(heights.length + 1);
  let acc = 0;
  for (let i = 0; i < heights.length; i += 1) {
    offsets[i] = acc;
    acc += heights[i] ?? ESTIMATED_ROW_HEIGHT;
  }
  offsets[heights.length] = acc;
  return offsets;
}

/** 二分找出第一个 top + height > y 的行下标 */
export function indexAt(offsets: readonly number[], y: number): number {
  const n = offsets.length - 1;
  if (n <= 0) return 0;
  let lo = 0;
  let hi = n;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    const top = offsets[mid] ?? 0;
    const bottom = offsets[mid + 1] ?? top;
    if (bottom <= y) lo = mid + 1;
    else hi = mid;
  }
  return Math.min(lo, n - 1);
}

export function computeRange(
  scrollTop: number,
  viewportH: number,
  offsets: readonly number[],
  overscan = 10,
): Range {
  const total = offsets.length - 1;
  if (total <= 0) return { start: 0, end: 0, padTop: 0, padBottom: 0 };

  const top = Math.max(0, scrollTop);
  const bottom = top + Math.max(0, viewportH);

  const start = Math.max(0, indexAt(offsets, top) - overscan);
  // end 是「不包含」的下标，所以先找 bottom 所在行，再 +1，然后再补 overscan
  const end = Math.min(total, indexAt(offsets, bottom) + 1 + overscan);

  const padTop = offsets[start] ?? 0;
  const padBottom = (offsets[total] ?? 0) - (offsets[end] ?? 0);

  return { start, end, padTop, padBottom: Math.max(0, padBottom) };
}

/**
 * 向上插入历史后，把滚动位置顶回去，使视口内容不跳。
 * @param anchorIndex 插入前视口顶部那一行的下标
 * @param insertedHeight 新插入内容的总高度
 */
export function anchorShift(insertedHeight: number, prevScrollTop: number): number {
  return Math.max(0, prevScrollTop + insertedHeight);
}

/**
 * 是否需要触发「向上加载更早一页」。
 * 阈值取 48px，避免用户没滚到顶就被反复触发。
 */
export function shouldLoadMore(scrollTop: number, threshold = 48): boolean {
  return scrollTop <= threshold;
}
