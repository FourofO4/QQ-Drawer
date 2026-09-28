/**
 * 面板尺寸拖拽的纯逻辑（优化 2）。
 *
 * 这一层只做两件事：把"指针位移"换算成"新尺寸"，以及按上下限夹取。
 * 不碰 IPC、不碰 DOM —— 拖拽的手感问题（rAF 节流、指针捕获）留在 `ui/ResizeGrip.tsx`。
 *
 * ⚠️ **上下限不在这里写死**。真上限取决于当前显示器的工作区（屏幕越窄越小），
 * 那只有 Rust 侧知道，由 `begin_resize` 带回来。这里只负责用它夹。
 */

export type ResizeAxis = 'w' | 'h' | 'both';

export interface Size {
  w: number;
  h: number;
}

export interface Limits {
  min: Size;
  max: Size;
}

export interface Delta {
  x: number;
  y: number;
}

/**
 * 夹到合法区间，并取整。
 *
 * 取整是必须的：`set_size` 收的是 `u32`，小数会在 IPC 序列化时被拒。
 */
export function clampSize(size: Size, limits: Limits): Size {
  const clamp = (v: number, lo: number, hi: number) =>
    Math.round(Math.min(Math.max(v, lo), hi));
  return {
    w: clamp(size.w, limits.min.w, limits.max.w),
    h: clamp(size.h, limits.min.h, limits.max.h),
  };
}

/**
 * 拖拽起点 + 指针位移 → 新尺寸。
 *
 * 用**位移**而不是指针绝对坐标，是因为窗口本身就在跟着动：拿绝对坐标当尺寸，
 * 窗口一动坐标也跟着变，会自我反馈成抖动。位移只依赖按下那一刻的原点，天然稳定。
 *
 * `axis` 决定哪几个方向有效：右边缘把手只改宽、下边缘只手改高、右下角两个都改
 * （手感上"只想调宽"时用角标会连带改高度，所以三个把手是必要的，不是冗余）。
 */
export function sizeFromDrag(
  axis: ResizeAxis,
  start: Size,
  delta: Delta,
  limits: Limits,
): Size {
  return clampSize(
    {
      w: axis === 'h' ? start.w : start.w + delta.x,
      h: axis === 'w' ? start.h : start.h + delta.y,
    },
    limits,
  );
}

/**
 * 指针位移（CSS 像素）→ 窗口尺寸位移（物理像素）。
 *
 * `set_size` 收的是 `PhysicalSize`，而 `clientX/Y` 是 CSS 像素，两者在高 DPI 屏上差一个
 * `devicePixelRatio`。不换算的话在 150% 缩放的屏幕上拖动只有手指行程的 2/3，
 * 表现为"拖不动"。
 */
export function toPhysical(delta: Delta, dpr: number): Delta {
  const k = Number.isFinite(dpr) && dpr > 0 ? dpr : 1;
  return { x: delta.x * k, y: delta.y * k };
}
