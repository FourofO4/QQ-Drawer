import { describe, expect, it } from 'vitest';
import {
  DEFAULT_BUBBLE_ALPHA,
  DEFAULT_PANEL_ALPHA,
  FLASH_FLOOR,
  PANEL_PRESETS,
  flashAlpha,
  panelRgb,
  readabilityK,
  themeVars,
} from './theme';

describe('可读性补偿 §3.7', () => {
  it('k = clamp((alpha - 0.30) / 0.60, 0, 1)', () => {
    expect(readabilityK(0.9)).toBeCloseTo(1, 5);
    expect(readabilityK(0.3)).toBeCloseTo(0, 5);
    expect(readabilityK(0.72)).toBeCloseTo(0.7, 5);
    expect(readabilityK(0.1)).toBe(0); // 夹到 0
    expect(readabilityK(1.5)).toBe(1); // 夹到 1
  });

  it('越透 → 底色越深（而不是单纯变透明）', () => {
    const solid = panelRgb(0.9);
    const light = panelRgb(0.72);
    const clear = panelRgb(0.45);
    // alpha ≥ 0.9 时 k = 1，正好是 §3.6 给的默认底色
    expect(solid).toEqual([44, 44, 42]);
    expect(light).toEqual([39, 39, 37]);
    expect(clear).toEqual([31, 31, 29]);
    expect(light[0]).toBeLessThan(solid[0]);
    expect(clear[0]).toBeLessThan(light[0]);
    // 最透时趋近 (26,26,25)
    expect(clear[0]).toBe(26 + Math.round(18 * 0.25));
  });

  it('关掉补偿时底色固定为 44/44/42', () => {
    expect(panelRgb(0.45, false)).toEqual([44, 44, 42]);
    expect(panelRgb(0.9, false)).toEqual([44, 44, 42]);
  });

  it('闪烁最暗时的 alpha = panel × 0.42，且不低过下限', () => {
    expect(flashAlpha(0.72)).toBeCloseTo(0.3024, 4);
    expect(flashAlpha(0.05)).toBe(FLASH_FLOOR);
  });
});

describe('主题变量输出', () => {
  const vars = themeVars({
    panelAlpha: DEFAULT_PANEL_ALPHA,
    bubbleAlpha: DEFAULT_BUBBLE_ALPHA,
    compensation: true,
  });

  it('输出的是完整颜色串，CSS 侧不再做运算（踩坑 #1）', () => {
    for (const [k, v] of Object.entries(vars)) {
      expect(v, k).not.toContain('calc(');
      expect(v, k).not.toContain('var(');
    }
  });

  it('面板底色串里的 alpha 与设置一致', () => {
    expect(vars['--panel-bg']).toBe(`rgba(39, 39, 37, 0.72)`);
  });

  it('面板层与气泡层各自独立，互不联动', () => {
    const wide = themeVars({ panelAlpha: 0.45, bubbleAlpha: 0.35, compensation: true });
    const narrow = themeVars({ panelAlpha: 0.45, bubbleAlpha: 0.35, compensation: true });
    expect(wide['--bub-own-bg']).toBe(narrow['--bub-own-bg']);
    expect(wide['--bub-own-bg']).toBe('rgba(55, 138, 221, 0.35)');
  });

  it('气泡 alpha 很低时描边不会消失（边界约束）', () => {
    const v = themeVars({ panelAlpha: 0.72, bubbleAlpha: 0.08, compensation: true });
    expect(v['--bub-other-stroke']).toBe('rgba(255, 255, 255, 0.13)');
  });

  it('溢出层底色近乎不透明，保证盖在消息上仍清晰', () => {
    expect(vars['--overflow-bg']).toBe('rgba(28, 28, 27, 0.95)');
  });
});

describe('预设档位', () => {
  it('三档预设为 0.90 / 0.72 / 0.45', () => {
    expect(PANEL_PRESETS.map((p) => p.value)).toEqual([0.9, 0.72, 0.45]);
    expect(PANEL_PRESETS[1]?.label).toBe('半透');
  });
});
