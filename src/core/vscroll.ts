/**
 * 虚拟滚动数学（NFR-05 / 优化清单 #1）。
 *
 * 只渲染可视区 ±overscan 行；累计高度表定位；视口位置用**锚点**保持不变。
 * 这里刻意不碰 DOM——DOM 测量在 MessageList 里做，本模块只负责算下标。
 *
 * ## 为什么必须有锚点
 *
 * `scrollTop` 是个绝对像素值，但它所在的内容坐标系（`offsets`）会变：
 * 行高从估值换成实测、向上翻页插入历史，都会改写 `offsets`。坐标系一变，
 * 同一个 `scrollTop` 就指向别的内容了 —— 用户看到的就是"消息自己往回退"。
 *
 * 真正稳定的量不是 `scrollTop`，而是**用户正看着哪一行、看到该行的第几像素**。
 * 把这个叫锚点，坐标系换代后由锚点反算新的 `scrollTop`，视口就一像素都不动。
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

/** 行高未知时的初始估值。测量样本攒够之后由 `estimateRowHeight` 接手 */
export const ESTIMATED_ROW_HEIGHT = 44;

/** 自适应估值至少要有这么多测量样本才敢用（样本太少中位数没意义） */
export const ESTIMATE_SAMPLE_MIN = 5;

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

/**
 * 未测量行的高度估值：取已测行高的**中位数**。
 *
 * 取中位数而不是均值：图片行动辄两三百像素，均值会被它整体拉偏，
 * 中位数对离群值免疫，误差能压到几像素。估值越准，渲染区的真实高度与
 * 未渲染区的占位高度就越一致，总高度（`scrollHeight`）越稳。
 *
 * ⚠️ **调用一次就够，不要每来一个新样本就重算。**
 *
 * 估值参与 `padTop` / `padBottom` 的计算，而 pad 决定 `scrollHeight`。若估值随样本
 * 浮动，**所有未渲染行**的高度会一起变 —— 500 行的列表里这一下就是几千像素的
 * `scrollHeight` 突变：滚动条长度跟着跳，渲染窗口的边界也大幅移动、引发新一波行测量，
 * 而新测量又改估值…… 形成自激，主线程被吃满，滚轮就不动了。
 *
 * 所以正确的用法是**只学一次**：首屏渲染拿到第一批样本就钉死。
 * 调用点见 `ui/MessageList.tsx` 的 `estimate`。
 */
export function estimateRowHeight(
  measured: readonly number[],
  fallback = ESTIMATED_ROW_HEIGHT,
): number {
  const xs = measured.filter((h) => Number.isFinite(h) && h > 0);
  if (xs.length < ESTIMATE_SAMPLE_MIN) return fallback;
  xs.sort((a, b) => a - b);
  const mid = xs.length >> 1;
  const median = xs.length % 2 === 1 ? xs[mid]! : (xs[mid - 1]! + xs[mid]!) / 2;
  return Math.round(median);
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

/** 视口锚点：钉住「哪一行、该行顶边往下多少像素」 */
export interface Anchor {
  /** 行下标（offsets 的坐标系） */
  index: number;
  /** 视口顶边相对该行顶边的偏移，恒 ≥ 0 */
  inner: number;
}

/** 由 scrollTop 取锚点。越界一律夹到合法范围，不抛错 */
export function anchorAt(offsets: readonly number[], scrollTop: number): Anchor {
  const total = offsets.length - 1;
  if (total <= 0) return { index: 0, inner: 0 };
  const y = Math.max(0, scrollTop);
  const index = indexAt(offsets, y);
  const top = offsets[index] ?? 0;
  return { index, inner: Math.max(0, y - top) };
}

/** 由锚点反算 scrollTop */
export function anchorTop(offsets: readonly number[], a: Anchor): number {
  const total = offsets.length - 1;
  if (total <= 0) return 0;
  const index = Math.max(0, Math.min(a.index, total - 1));
  return Math.max(0, (offsets[index] ?? 0) + a.inner);
}

/**
 * 取视口当前锚在**哪一行的 key** 上、行内偏移多少。
 *
 * 用 key 而不是下标：向上翻页会让所有下标整体后移，下标在新行序里对不上，
 * 而 message_id 是稳定的。行序为空时返回 null。
 */
export function anchorKeyAt(
  offsets: readonly number[],
  keys: readonly string[],
  scrollTop: number,
): { key: string; inner: number } | null {
  if (offsets.length !== keys.length + 1) return null;
  const a = anchorAt(offsets, scrollTop);
  const key = keys[a.index];
  return key === undefined ? null : { key, inner: a.inner };
}

/**
 * 由「锚点行的 key + 行内偏移」反算 scrollTop。
 * 该 key 在新行序里找不到时返回 null，交给调用方决定怎么兜底。
 */
export function topForKey(
  offsets: readonly number[],
  keys: readonly string[],
  key: string,
  inner: number,
): number | null {
  if (offsets.length !== keys.length + 1) return null;
  const index = keys.indexOf(key);
  if (index < 0) return null;
  return anchorTop(offsets, { index, inner });
}

/**
 * 本次行序变化是不是「向下追加」（新消息进列表）。
 *
 * 只有这种情况才该把视口跟到底部。向上翻页插入历史时首行会变，判为 false ——
 * 若在这里跟到底部，就会出现「用户往上翻历史，翻一页被弹回底部」的鬼打墙。
 */
export function isAppendOnly(
  before: readonly string[],
  after: readonly string[],
): boolean {
  if (before.length === 0 || after.length <= before.length) return false;
  // 首行换了人 → 前面插了东西（翻页），不是追加
  return before[0] === after[0];
}

/**
 * 是否需要触发「向上加载更早一页」。
 * 阈值取 48px，避免用户没滚到顶就被反复触发。
 */
export function shouldLoadMore(scrollTop: number, threshold = 48): boolean {
  return scrollTop <= threshold;
}
