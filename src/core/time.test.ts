import { describe, expect, it } from 'vitest';
import {
  SEPARATOR_GAP_MS,
  formatClock,
  formatDay,
  prependScrollTop,
  sameDay,
  separatorFor,
} from './time';

const NOW = new Date(2026, 8, 17, 14, 2, 0).getTime(); // 2026-09-17 周四 14:02

describe('时间文案', () => {
  it('两位补零的 HH:MM', () => {
    expect(formatClock(new Date(2026, 8, 17, 9, 5).getTime())).toBe('09:05');
    expect(formatClock(new Date(2026, 8, 17, 23, 59).getTime())).toBe('23:59');
  });

  it('当天显示"今天"、前一天"昨天"、再前一天"前天"', () => {
    expect(formatDay(NOW, NOW)).toBe('今天');
    expect(formatDay(new Date(2026, 8, 16, 23, 0).getTime(), NOW)).toBe('昨天');
    expect(formatDay(new Date(2026, 8, 15, 1, 0).getTime(), NOW)).toBe('前天');
  });

  it('一周内用星期', () => {
    // 2026-09-17 是周四，往前 4 天是 13 号周日
    expect(formatDay(new Date(2026, 8, 13, 12, 0).getTime(), NOW)).toBe('周日');
    expect(formatDay(new Date(2026, 8, 14, 12, 0).getTime(), NOW)).toBe('周一');
  });

  it('同年超过一周用「M月D日」', () => {
    expect(formatDay(new Date(2026, 7, 1, 12, 0).getTime(), NOW)).toBe('8月1日');
  });

  it('跨年带上年份', () => {
    expect(formatDay(new Date(2025, 11, 31, 12, 0).getTime(), NOW)).toBe('2025年12月31日');
  });

  it('同一天判定', () => {
    expect(sameDay(NOW, new Date(2026, 8, 17, 0, 1).getTime())).toBe(true);
    expect(sameDay(NOW, new Date(2026, 8, 16, 23, 59).getTime())).toBe(false);
  });
});

describe('分隔规则 FR-19', () => {
  it('本页第一条必定带日期分隔（并附时刻）', () => {
    expect(separatorFor(null, NOW, NOW)).toEqual({ kind: 'day', text: '今天 14:02' });
  });

  it('跨天用日期分隔', () => {
    const prev = new Date(2026, 8, 16, 22, 40).getTime();
    expect(separatorFor(prev, NOW, NOW)).toEqual({ kind: 'day', text: '今天 14:02' });
  });

  it('同天超过 5 分钟用时刻分隔', () => {
    const prev = new Date(2026, 8, 17, 13, 50).getTime();
    expect(separatorFor(prev, NOW, NOW)).toEqual({ kind: 'time', text: '14:02' });
  });

  it('恰好 5 分钟不插——超过才插', () => {
    const exact = NOW - SEPARATOR_GAP_MS;
    expect(separatorFor(exact, NOW, NOW)).toBeNull();
    expect(separatorFor(exact - 1, NOW, NOW)).toEqual({ kind: 'time', text: '14:02' });
  });

  it('5 分钟以内不插分隔', () => {
    expect(separatorFor(NOW - 60_000, NOW, NOW)).toBeNull();
  });
});

describe('翻页滚动位置', () => {
  it('插入高度直接加到 scrollTop 上', () => {
    expect(prependScrollTop(120, 400)).toBe(520);
  });

  it('结果不为负', () => {
    expect(prependScrollTop(0, 0)).toBe(0);
  });
});
