import { describe, expect, it } from 'vitest';
import {
  buildOffsets,
  computeRange,
  indexAt,
  anchorShift,
  shouldLoadMore,
  ESTIMATED_ROW_HEIGHT,
} from './vscroll';

describe('累计高度表', () => {
  it('首元素为 0，末元素等于总高度', () => {
    expect(buildOffsets([10, 20, 30])).toEqual([0, 10, 30, 60]);
  });

  it('空列表只有一个 0', () => {
    expect(buildOffsets([])).toEqual([0]);
  });

  it('行高缺失时用估值兜底', () => {
    const offsets = buildOffsets([10, undefined as unknown as number, 10]);
    expect(offsets[3]).toBe(20 + ESTIMATED_ROW_HEIGHT);
  });
});

describe('二分定位', () => {
  const offsets = buildOffsets([10, 10, 10, 10]); // 0,10,20,30,40

  it('命中各行顶部', () => {
    expect(indexAt(offsets, 0)).toBe(0);
    expect(indexAt(offsets, 10)).toBe(1);
    expect(indexAt(offsets, 25)).toBe(2);
  });

  it('超出总高时夹到最后一行', () => {
    expect(indexAt(offsets, 9999)).toBe(3);
  });

  it('空表返回 0', () => {
    expect(indexAt([0], 100)).toBe(0);
  });
});

describe('可视区计算 NFR-05', () => {
  const heights = new Array(100).fill(20); // 总高 2000
  const offsets = buildOffsets(heights);

  it('只渲染可视区 ± overscan，不渲染全部', () => {
    const r = computeRange(500, 200, offsets, 10);
    // 500~700 对应第 25~35 行，前后各扩 10
    expect(r.start).toBe(15);
    expect(r.end).toBe(46);
  });

  it('顶部时 start 不小于 0，上方占位为 0', () => {
    const r = computeRange(0, 200, offsets, 10);
    expect(r.start).toBe(0);
    expect(r.padTop).toBe(0);
  });

  it('底部时 end 不超过总行数，下方占位为 0', () => {
    const r = computeRange(1800, 200, offsets, 10);
    expect(r.end).toBe(100);
    expect(r.padBottom).toBe(0);
  });

  it('上下占位之和 + 渲染高度 = 总高度（保证滚动条不跳）', () => {
    const r = computeRange(743, 260, offsets, 10);
    const rendered = offsets[r.end]! - offsets[r.start]!;
    expect(r.padTop + rendered + r.padBottom).toBe(offsets[100]);
  });

  it('空列表不崩', () => {
    expect(computeRange(0, 200, buildOffsets([]), 10)).toEqual({
      start: 0,
      end: 0,
      padTop: 0,
      padBottom: 0,
    });
  });
});

describe('翻页与触底判定', () => {
  it('插入历史后把滚动位置顶回原处，视口不跳', () => {
    expect(anchorShift(320, 40)).toBe(360);
  });

  it('插入高度为 0 时滚动位置不变', () => {
    expect(anchorShift(0, 40)).toBe(40);
  });

  it('贴到顶部阈值内才触发加载更早的一页', () => {
    expect(shouldLoadMore(0)).toBe(true);
    expect(shouldLoadMore(48)).toBe(true);
    expect(shouldLoadMore(49)).toBe(false);
  });
});
