/**
 * 面板拖拽的纯逻辑（优化 2）。
 *
 * 这一层只做一件事：把**指针位移**换算成**新的面板矩形**。
 * 不碰 IPC、不碰 DOM —— 手感问题（rAF 合并、指针捕获、坐标系）留在 `ui/ResizeGrip.tsx`。
 *
 * ---- 为什么算的是"矩形"而不是"尺寸" ----
 *
 * 一开始这里只有 `sizeFromDrag`（给定起点尺寸 + 位移 → 新尺寸），位置交给 Rust 按锚点推。
 * 那样**只有右 / 下两条边能用**：拖左、上边缘时窗口原点也得跟着走，而"原点该走多少"
 * 是尺寸的函数，前端算不出来。硬要支持就只剩"改尺寸、原点不动"——
 * 表现为**拖上边界、下边界在动**。
 *
 * 所以这里改成算完整矩形：拖哪条边，**对边钉住**（拖右边缘时左边缘不动，反之亦然）。
 * 这正是原生窗口的行为，也是用户的固有预期。
 *
 * ⚠️ 函数必须是**纯函数**：只依赖 `start` + `delta`，绝不能去读"窗口现在在哪"。
 * 读窗口当前位置会形成正反馈 —— 拖拽会移动窗口 → 窗口一动"指针相对窗口的坐标"就变 →
 * 下一个位移更大 → 窗口移动更多。上一版就是这样：拖右边把手会让面板一路缩到
 * 360×500 那种竖屏比例，而且松手就把这个错值写进了库。
 * 位移由 `ResizeGrip` 用**屏幕坐标**（`screenX/screenY`）测量，与窗口位置无关。
 */

/** 抓的是哪条边 / 哪个角。左、上的字母表示"那条边跟着指针走"。 */
export type Edge = 'n' | 's' | 'e' | 'w' | 'ne' | 'nw' | 'se' | 'sw';

export interface Size {
  w: number;
  h: number;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Delta {
  x: number;
  y: number;
}

/** 工作区（物理像素）。上限就隐含在它里面：面板不能超出工作区减去两侧边距。 */
export interface WorkArea {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * 拖拽期间的可拖范围。
 *
 * ⚠️ **上下限不在这里写死**。真上限取决于当前显示器的工作区（屏幕越窄越小），
 * 那只有 Rust 侧知道，由 `begin_resize` 带回来。
 */
export interface DragBounds {
  min: Size;
  work: WorkArea;
  margin: number;
}

/**
 * 夹进 `[lo, hi]`。
 *
 * 区间可能反转（小屏下工作区比下限还小），这时以 `lo` 为准 —— 宁可让面板撑出屏幕，
 * 也不要算出零宽或者负数尺寸的窗口。
 */
function clamp(v: number, lo: number, hi: number): number {
  return Math.min(Math.max(v, lo), Math.max(lo, hi));
}

/**
 * 拖拽起点 + 指针位移 → 新矩形。**对边钉住**，移动的那条边夹在工作区内。
 *
 * 两个边界行为是刻意的：
 *  - 面板已经贴住屏幕右缘时，继续往右拖**什么都不发生**（原生窗口同样如此），
 *    而不是钻到屏幕外面去，也不是反过来把左边缘推走；
 *  - 缩到下限尺寸就停住，不会"翻面"（宽度变负）。
 *
 * 两个轴向互相独立：拖 `n` 只动上边，`w` 只动左边，`nw` 两个都动。
 */
export function rectFromDrag(
  edge: Edge,
  start: Rect,
  delta: Delta,
  bounds: DragBounds,
): Rect {
  const { margin, min } = bounds;
  const left = bounds.work.x + margin;
  const top = bounds.work.y + margin;
  const right = bounds.work.x + bounds.work.w - margin;
  const bottom = bounds.work.y + bounds.work.h - margin;

  let { x, y, w, h } = start;
  /** 按下时对面的那条边 —— 整个拖拽过程中它都是常量。 */
  const right0 = x + w;
  const bottom0 = y + h;

  if (edge.includes('w')) {
    // 拖左边：右边缘钉住
    const nx = clamp(x + delta.x, left, right0 - min.w);
    x = nx;
    w = right0 - nx;
  } else if (edge.includes('e')) {
    // 拖右边：左边缘钉住
    w = clamp(right0 + delta.x, x + min.w, right) - x;
  }

  if (edge.includes('n')) {
    const ny = clamp(y + delta.y, top, bottom0 - min.h);
    y = ny;
    h = bottom0 - ny;
  } else if (edge.includes('s')) {
    h = clamp(bottom0 + delta.y, y + min.h, bottom) - y;
  }

  return { x: Math.round(x), y: Math.round(y), w: Math.round(w), h: Math.round(h) };
}

/**
 * 指针位移（CSS 像素）→ 窗口位移（物理像素）。
 *
 * `set_size` / `set_position` 收的是物理像素，而指针事件的坐标是 CSS 像素，
 * 两者在高 DPI 屏上差一个 `devicePixelRatio`。不换算的话在 150% 缩放的屏幕上
 * 拖动只有手指行程的 2/3，表现为"拖不动"。
 */
export function toPhysical(delta: Delta, dpr: number): Delta {
  const k = Number.isFinite(dpr) && dpr > 0 ? dpr : 1;
  return { x: delta.x * k, y: delta.y * k };
}
