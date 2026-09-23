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

/**
 * 用消息 id 表达的锚点。
 *
 * 翻页会让所有下标整体后移，下标在新行序里对不上；`message_id` 不会。所以跨
 * 坐标系搬运视口位置时一律用这个形式。
 */
export interface AnchorKey {
  key: string;
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
): AnchorKey | null {
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

/**
 * 把渲染窗口里**同步量到**的真实行高合并进高度表。
 *
 * 为什么要有同步测量这一步：`offsets` 是算视口位置的坐标系，而 DOM 里的真实高度
 * 只有 `offsetHeight` 说了算。只要还有「已渲染但没测量」的行，`offsets` 与真实布局
 * 就对不上 —— 这时用 `offsets` 反算出来的锚点位置本身就是错的，补完还是歪的。
 * 翻页插入一页新行时最明显：那批行全是估值（比如 44px），真实行高可能是 60px，
 * 三十行差 480px，于是「先错一帧、等 ResizeObserver 回填再跳回来」——用户看到的就是闪。
 * 同步读一遍 `offsetHeight` 会让浏览器立刻 layout 并把真实高度交出来，这个偏差当场消失。
 *
 * 返回值里 `changed` 很关键：**没变就必须原样返回旧对象**。每写一次高度信号就是一轮
 * 渲染 + 一轮视口校正，白白触发就是白白抖动。
 */
export function mergeMeasured(
  heights: Readonly<Record<string, number>>,
  measured: readonly { key: string; height: number }[],
): { heights: Record<string, number>; changed: boolean } {
  let next: Record<string, number> | null = null;
  for (const { key, height } of measured) {
    // 行还没进布局（`display:none`、被移除）时会量到 0，这种样本必须丢掉：
    // 收下来会把这一行的高度记成 0，坐标系出现一个洞。
    if (key === '' || !Number.isFinite(height) || height <= 0) continue;
    const cur = next === null ? heights[key] : next[key];
    if (cur === height) continue;
    if (next === null) next = { ...heights };
    next[key] = height;
  }
  return next === null ? { heights: heights as Record<string, number>, changed: false } : { heights: next, changed: true };
}

/** 坐标系换代后视口该怎么走 */
export type ViewportAction =
  /** 跳到底部：换会话、或用户本来就贴着底、又来了新消息 */
  | 'jump-bottom'
  /** 钉住锚点：把「用户正看着的那一行」放回视口里原来的位置 */
  | 'pin'
  /** 什么都不做：行序与行高都没变，此刻的 `scrollTop` 就是用户的意图 */
  | 'hold';

/**
 * 决定视口动作。这是 bug 3 里最容易写错的一处判定，所以单独抽出来钉住。
 *
 * 三条判据的由来：
 * - **`coordChanged` 为假就 `hold`**。用户的滚动也会被响应式系统看到，但它不改行序、
 *   也不改行高 —— 坐标系没换代时去「校正」视口，就是把用户刚滚出来的位置又拽回去。
 * - **贴底跟随只认「向下追加」**。翻页是往前面插历史，首行换了人；这时跟到底部，
 *   就是「往上翻一页被弹回底部」的鬼打墙。所以翻页（`paging`）期间一律 `pin`。
 * - **`paging` 期间用户可能已经滚到别处**，`pin` 用的是他此刻的位置，不是翻页前那个。
 * - **行序没动、只有行高变了**（图片解码完、字体换行算准了）也要跟：贴底时那是新内容
 *   把底部往下推，不跟的话用户就停在半截。所以只有**行序变化**才需要分追加与前插。
 */
export function planViewport(o: {
  /** 行序整个换了一批（换会话） */
  peerChanged: boolean;
  /** 首屏还没定位过 */
  jumpPending: boolean;
  /** `offsets` 或行序变了 —— 坐标系换代 */
  coordChanged: boolean;
  /** 行序本身变了（前插会让首行换人） */
  orderChanged: boolean;
  /** 行序是「首行不动、尾部变长」的向下追加 */
  appended: boolean;
  /** 用户此刻贴着底部 */
  pinned: boolean;
  /** 正在向上翻页 */
  paging: boolean;
}): ViewportAction {
  if (o.peerChanged || o.jumpPending) return 'jump-bottom';
  if (!o.coordChanged) return 'hold';
  // 翻页期间这个 effect 让路：位置的补偿由翻页流程自己做（`ui/MessageList.tsx` 的
  // `paginateUp`）。它既持有跨 await 取好的锚点，又跑在 async 上下文里 ——
  // 那里写信号能同步推进 DOM，测量/补偿的迭代才真的收敛；而 effect 执行期间
  // Solid 的更新队列是锁的，迭代第二轮量到的还是同一批 DOM，补出来的位置是偏的。
  if (o.paging) return 'hold';
  // 没贴底就是在看历史，钉住别动。翻页已经在上一条让开了，这里的 `pinned` 是
  // "用户真的停在底部"的那一次判定。
  if (!o.pinned) return 'pin';
  // 贴底：只有「往前面插了东西」才不能跟（那种情况首行会换人，见 `isAppendOnly`）
  if (!o.orderChanged || o.appended) return 'jump-bottom';
  return 'pin';
}
