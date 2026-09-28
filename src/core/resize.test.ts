import { describe, expect, it } from 'vitest';
import { rectFromDrag, toPhysical, type DragBounds, type Rect } from './resize';

const work = { x: 0, y: 0, w: 1920, h: 1080 };

const bounds = (over: Partial<DragBounds> = {}): DragBounds => ({
  min: { w: 360, h: 240 },
  work,
  margin: 16,
  ...over,
});

/** 常用起点：584×500 摆在工作区中段，四边都不贴边 */
const start: Rect = { x: 600, y: 300, w: 584, h: 500 };

describe('rectFromDrag：抓哪条边就动哪条边，对边钉住', () => {
  it('拖右边只改右边，左边不动', () => {
    expect(rectFromDrag('e', start, { x: 100, y: 60 }, bounds())).toEqual({
      x: 600,
      y: 300,
      w: 684,
      h: 500,
    });
  });

  it('拖左边时右边钉住 —— 这正是不再"改尺寸、原点不动"的理由', () => {
    const r = rectFromDrag('w', start, { x: 100, y: 60 }, bounds());
    expect(r).toEqual({ x: 700, y: 300, w: 484, h: 500 });
    expect(r.x + r.w).toBe(start.x + start.w);
  });

  it('拖下边只改高，上边不动', () => {
    expect(rectFromDrag('s', start, { x: 100, y: 60 }, bounds())).toEqual({
      x: 600,
      y: 300,
      w: 584,
      h: 560,
    });
  });

  it('拖上边时下边钉住', () => {
    const r = rectFromDrag('n', start, { x: 0, y: -60 }, bounds());
    expect(r).toEqual({ x: 600, y: 240, w: 584, h: 560 });
    expect(r.y + r.h).toBe(start.y + start.h);
  });

  it('角上两个轴一起动，另外两条边钉住', () => {
    expect(rectFromDrag('nw', start, { x: -40, y: -30 }, bounds())).toEqual({
      x: 560,
      y: 270,
      w: 624,
      h: 530,
    });
  });
});

describe('rectFromDrag：夹取', () => {
  it('贴住屏幕右缘后再往右拖什么都不发生（而不是把左边缘推走）', () => {
    const flush: Rect = { x: 1920 - 16 - 584, y: 300, w: 584, h: 500 };
    expect(rectFromDrag('e', flush, { x: 200, y: 0 }, bounds())).toEqual(flush);
  });

  it('拖右边到屏幕外时宽度被截在边距上', () => {
    const r = rectFromDrag('e', start, { x: 9999, y: 0 }, bounds());
    expect(r.x).toBe(600);
    expect(r.w).toBe(1920 - 16 - 600);
  });

  it('拖左边到屏幕外时左边缘停在边距上，宽度跟着缩', () => {
    const r = rectFromDrag('w', start, { x: -9999, y: 0 }, bounds());
    expect(r.x).toBe(16);
    expect(r.x + r.w).toBe(start.x + start.w);
  });

  it('缩到下限就停，不会翻面', () => {
    const r = rectFromDrag('se', start, { x: -9999, y: -9999 }, bounds());
    expect(r.w).toBe(360);
    expect(r.h).toBe(240);
  });

  it('拖左上角越过右下角时以下限为准，不会出现负尺寸', () => {
    const r = rectFromDrag('nw', start, { x: 9999, y: 9999 }, bounds());
    expect(r.w).toBe(360);
    expect(r.h).toBe(240);
  });

  it('工作区比下限还小时以下限为准（不给零宽窗口）', () => {
    const tiny = bounds({ work: { x: 0, y: 0, w: 200, h: 100 } });
    const r = rectFromDrag('e', { x: 16, y: 16, w: 360, h: 240 }, { x: 9999, y: 9999 }, tiny);
    expect(r.w).toBe(360);
    expect(r.h).toBe(240);
  });
});

describe('rectFromDrag：结果只由起点与位移决定（正反馈回归）', () => {
  // 上一版的 bug：位移用 clientX（相对窗口）测量，而拖拽本身在移动窗口，于是
  // "窗口动 → 指针相对坐标变 → 位移变大 → 窗口动更多"自我放大，拖一下右边把手
  // 就把面板缩成 360×500 那种竖屏比例，松手还把这个错值写进了库。
  // 结构上的防线是：这个函数**只吃 start + delta**，没有任何"窗口现在在哪"的输入。
  it('同起点 + 同位移 → 同一结果，可重复', () => {
    expect(rectFromDrag('e', start, { x: 300, y: 0 }, bounds())).toEqual(
      rectFromDrag('e', start, { x: 300, y: 0 }, bounds()),
    );
  });

  it('位移拆成多步累积，与一步到位一致（不会自我放大）', () => {
    let r = start;
    for (let i = 0; i < 5; i += 1) r = rectFromDrag('e', r, { x: 20, y: 0 }, bounds());
    expect(r).toEqual(rectFromDrag('e', start, { x: 100, y: 0 }, bounds()));
  });
});

describe('toPhysical：CSS 像素 → 物理像素', () => {
  it('100% 缩放就是原值', () => {
    expect(toPhysical({ x: 10, y: -4 }, 1)).toEqual({ x: 10, y: -4 });
  });

  it('150% 缩放要放大 1.5 倍（否则拖动只有行程的 2/3）', () => {
    expect(toPhysical({ x: 10, y: -4 }, 1.5)).toEqual({ x: 15, y: -6 });
  });

  it('拿不到 dpr 时按 1 处理', () => {
    expect(toPhysical({ x: 10, y: 4 }, Number.NaN)).toEqual({ x: 10, y: 4 });
    expect(toPhysical({ x: 10, y: 4 }, 0)).toEqual({ x: 10, y: 4 });
  });
});
