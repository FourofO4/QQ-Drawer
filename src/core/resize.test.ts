import { describe, expect, it } from 'vitest';
import { clampSize, sizeFromDrag, toPhysical, type Limits } from './resize';

const limits: Limits = { min: { w: 360, h: 240 }, max: { w: 1888, h: 1048 } };

describe('面板尺寸拖拽（优化 2）', () => {
  describe('sizeFromDrag：位移 → 新尺寸', () => {
    it('右下角把手：宽高一起跟位移走', () => {
      expect(
        sizeFromDrag('both', { w: 584, h: 500 }, { x: 100, y: 60 }, limits),
      ).toEqual({ w: 684, h: 560 });
    });

    it('右边缘把手：高度纹丝不动', () => {
      // 用角标只想调宽的时候高度也会动，手感是坏的 —— 三个把手存在的理由
      expect(sizeFromDrag('w', { w: 584, h: 500 }, { x: 100, y: 60 }, limits)).toEqual({
        w: 684,
        h: 500,
      });
    });

    it('下边缘把手：宽度纹丝不动', () => {
      expect(sizeFromDrag('h', { w: 584, h: 500 }, { x: 100, y: 60 }, limits)).toEqual({
        w: 584,
        h: 560,
      });
    });

    it('往回拖（负位移）会变小 —— 缩到下限为止', () => {
      expect(sizeFromDrag('both', { w: 584, h: 500 }, { x: -100, y: -60 }, limits)).toEqual({
        w: 484,
        h: 440,
      });
      expect(
        sizeFromDrag('both', { w: 584, h: 500 }, { x: -9999, y: -9999 }, limits),
      ).toEqual({ w: 360, h: 240 });
    });

    it('顶到上限也不会越界', () => {
      expect(sizeFromDrag('both', { w: 584, h: 500 }, { x: 9999, y: 9999 }, limits)).toEqual({
        w: 1888,
        h: 1048,
      });
    });

    it('位移为 0 时尺寸不变（点一下不产生写库）', () => {
      expect(sizeFromDrag('both', { w: 720, h: 640 }, { x: 0, y: 0 }, limits)).toEqual({
        w: 720,
        h: 640,
      });
    });
  });

  describe('clampSize：取整与夹取', () => {
    it('小数被取整（set_size 收的是 u32，小数会在序列化时被拒）', () => {
      expect(clampSize({ w: 584.4, h: 500.6 }, limits)).toEqual({ w: 584, h: 501 });
    });

    it('界内原样返回', () => {
      expect(clampSize({ w: 800, h: 600 }, limits)).toEqual({ w: 800, h: 600 });
    });

    it('上下限本身是闭区间端点', () => {
      expect(clampSize({ w: 360, h: 240 }, limits)).toEqual({ w: 360, h: 240 });
      expect(clampSize({ w: 1888, h: 1048 }, limits)).toEqual({ w: 1888, h: 1048 });
    });
  });

  describe('toPhysical：CSS 像素 → 物理像素', () => {
    it('100% 缩放时原样不动', () => {
      expect(toPhysical({ x: 100, y: 60 }, 1)).toEqual({ x: 100, y: 60 });
    });

    it('150% 缩放时按比例放大 —— 不换算会让拖动只有行程的 2/3', () => {
      expect(toPhysical({ x: 100, y: 60 }, 1.5)).toEqual({ x: 150, y: 90 });
    });

    it('dpr 缺失 / 非法时退化成 1，而不是算出 NaN', () => {
      // 退化成 1 顶多是手感偏一点；算出 NaN 会让窗口尺寸直接崩掉
      expect(toPhysical({ x: 10, y: 10 }, 0)).toEqual({ x: 10, y: 10 });
      expect(toPhysical({ x: 10, y: 10 }, Number.NaN)).toEqual({ x: 10, y: 10 });
      expect(toPhysical({ x: 10, y: 10 }, -1)).toEqual({ x: 10, y: 10 });
    });
  });
});
