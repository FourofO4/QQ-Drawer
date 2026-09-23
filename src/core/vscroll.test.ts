import { describe, expect, it } from 'vitest';
import {
  ESTIMATE_SAMPLE_MIN,
  ESTIMATED_ROW_HEIGHT,
  anchorAt,
  anchorKeyAt,
  anchorTop,
  buildOffsets,
  computeRange,
  estimateRowHeight,
  indexAt,
  isAppendOnly,
  shouldLoadMore,
  topForKey,
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

describe('行高估值：未测行按已测行的中位数估', () => {
  it('样本不够时退回固定估值（几行的中位数没有意义）', () => {
    const few = new Array(ESTIMATE_SAMPLE_MIN - 1).fill(70);
    expect(estimateRowHeight(few)).toBe(ESTIMATED_ROW_HEIGHT);
    expect(estimateRowHeight([])).toBe(ESTIMATED_ROW_HEIGHT);
  });

  it('样本够了就取中位数，不再用 44', () => {
    expect(estimateRowHeight([70, 72, 68, 70, 70, 70])).toBe(70);
  });

  it('单条超高的图片行不会把估值拉偏（中位数抗离群）', () => {
    // 均值会被那条 300px 的图片行拉到 108，中位数仍是 70
    expect(estimateRowHeight([70, 72, 68, 70, 70, 300])).toBe(70);
  });

  it('偶数样本取中间两个的平均', () => {
    expect(estimateRowHeight([60, 62, 70, 72, 80, 82])).toBe(71);
  });

  it('非法的测量值（0 / 负数 / NaN）不参与统计', () => {
    expect(estimateRowHeight([0, -5, Number.NaN, 70, 70, 70, 70, 70])).toBe(70);
  });

  it('估值贴近真实行高时，滑动渲染窗口不会让总高度抽动', () => {
    const real = 70;
    const sample = new Array(12).fill(real);
    const est = estimateRowHeight(sample);
    // 只测了前 12 行，其余 88 行用估值 —— 总高度仍等于真实总高
    const offsets = buildOffsets(
      new Array(100).fill(0).map((_, i) => (i < 12 ? real : est)),
    );
    expect(offsets[100]).toBe(100 * real);
  });
});

describe('视口锚定：坐标系换代时视口内容不动', () => {
  const offsets = buildOffsets([10, 10, 10, 10]); // 0,10,20,30,40

  it('锚点取到「第几行 + 行内偏移」', () => {
    expect(anchorAt(offsets, 0)).toEqual({ index: 0, inner: 0 });
    expect(anchorAt(offsets, 5)).toEqual({ index: 0, inner: 5 });
    expect(anchorAt(offsets, 25)).toEqual({ index: 2, inner: 5 });
  });

  it('越界与负值一律夹到合法范围，不抛错', () => {
    expect(anchorAt(offsets, -100)).toEqual({ index: 0, inner: 0 });
    expect(anchorAt(offsets, 45)).toEqual({ index: 3, inner: 15 });
    // 超出总高一律夹到最后一行，不是抛错也不是越界
    expect(anchorAt(offsets, 9999).index).toBe(3);
    expect(anchorAt([0], 100)).toEqual({ index: 0, inner: 0 });
    expect(anchorTop([0], { index: 5, inner: 3 })).toBe(0);
  });

  it('anchorTop 与 anchorAt 互逆', () => {
    for (const y of [0, 3, 10, 17, 33, 40]) {
      expect(anchorTop(offsets, anchorAt(offsets, y))).toBe(y);
    }
  });

  it('按 key 取锚点，行序为空或长度对不上时返回 null', () => {
    const keys = ['a', 'b', 'c', 'd'];
    expect(anchorKeyAt(offsets, keys, 25)).toEqual({ key: 'c', inner: 5 });
    expect(anchorKeyAt(offsets, [], 25)).toBeNull();
    expect(anchorKeyAt(offsets, keys.slice(0, 2), 25)).toBeNull();
  });

  it('行高从估值换成实测后，原来那行仍停在视口顶部（消息不回退）', () => {
    const keys = ['a', 'b', 'c', 'd'];
    const before = buildOffsets([44, 44, 44, 44]); // 0,44,88,132,176
    const anchor = anchorKeyAt(before, keys, 88);
    expect(anchor).toEqual({ key: 'c', inner: 0 });

    // 上面两行实测出来是 70，坐标系整体被改写
    const after = buildOffsets([70, 70, 44, 44]); // 0,70,140,184,228
    const top = topForKey(after, keys, anchor!.key, anchor!.inner);

    // 视口顶依然贴着 c 行 —— 这正是 bug 3「滚着滚着消息回退」的修法
    expect(top).toBe(after[2]);
    expect(top).toBe(140);
  });

  it('向上翻页插入一页后，原来看着的那行仍停在视口顶部', () => {
    const beforeKeys = ['c', 'd', 'e'];
    const before = buildOffsets([44, 44, 44]); // 0,44,88,132
    const anchor = anchorKeyAt(before, beforeKeys, 44)!; // 锚住 d
    expect(anchor.key).toBe('d');

    const afterKeys = ['a', 'b', 'c', 'd', 'e'];
    const after = buildOffsets([44, 44, 44, 44, 44]); // d 顶部 = 132
    const top = topForKey(after, afterKeys, anchor.key, anchor.inner);

    expect(top).toBe(132);
    // scrollTop 被顶回原处，而不是停在 44 —— 视口才不会往旧消息那边窜
    expect(top!).toBeGreaterThan(44);
  });

  it('锚点行在新行序里找不到时不猜，返回 null 让调用方兜底', () => {
    const offs = buildOffsets([44, 44]);
    expect(topForKey(offs, ['x', 'y'], 'gone', 0)).toBeNull();
  });
});

describe('追加与前插的区分', () => {
  it('首行不变、行数变多 → 是追加（该跟到底部）', () => {
    expect(isAppendOnly(['a', 'b'], ['a', 'b', 'c'])).toBe(true);
  });

  it('首行换了 → 是前插，绝不能跟到底部', () => {
    expect(isAppendOnly(['c', 'd'], ['a', 'b', 'c', 'd'])).toBe(false);
  });

  it('行数没变多（更新 / 清空 / 换会话）都不是追加', () => {
    expect(isAppendOnly(['a', 'b'], ['a', 'b'])).toBe(false);
    expect(isAppendOnly(['a', 'b'], ['a'])).toBe(false);
    expect(isAppendOnly([], ['a'])).toBe(false);
  });

  it('只有一条消息时追加也算追加', () => {
    expect(isAppendOnly(['a'], ['a', 'b'])).toBe(true);
  });
});

describe('触底判定', () => {
  it('贴到顶部阈值内才触发加载更早的一页', () => {
    expect(shouldLoadMore(0)).toBe(true);
    expect(shouldLoadMore(48)).toBe(true);
    expect(shouldLoadMore(49)).toBe(false);
  });
});
