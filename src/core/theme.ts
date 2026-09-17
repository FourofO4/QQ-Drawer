/**
 * 透明度模型与可读性补偿（§3.7）。
 *
 * ⚠️ 这里是踩坑清单第 1 条的正解：`calc()` **不能**作为 `rgba()` 的 alpha 参数，
 * 在 Edge/WebView2 上实测完全不生效（背景会整块透明、文字直接穿透）。
 * 所以所有颜色都在 JS 里算成完整字符串，再写进 CSS 变量；CSS 只读变量，不做运算。
 *
 * Rust 侧 `window.rs::readability` 有同一套公式的权威实现，两边必须保持一致。
 */

/** 三档预设：清晰 / 半透（默认） / 通透 */
export const PANEL_PRESETS = [
  { label: '清晰', value: 0.9 },
  { label: '半透', value: 0.72 },
  { label: '通透', value: 0.45 },
] as const;

export const DEFAULT_PANEL_ALPHA = 0.72;
export const DEFAULT_BUBBLE_ALPHA = 0.35;

/** 闪烁最暗时的面板 alpha 系数，下限 0.06 */
export const FLASH_FACTOR = 0.42;
export const FLASH_FLOOR = 0.06;

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

/** k = clamp((alpha - 0.30) / 0.60, 0, 1)；alpha 越小 k 越小 */
export function readabilityK(alpha: number): number {
  return clamp((alpha - 0.3) / 0.6, 0, 1);
}

/**
 * 面板底色 RGB。面板越透 → 底色越深，而不是单纯变透明（§3.7 关键规则）。
 * 注意：这是**补偿开**的结果；关掉补偿时固定用 44/44/42。
 */
export function panelRgb(alpha: number, compensation = true): [number, number, number] {
  if (!compensation) return [44, 44, 42];
  const k = readabilityK(alpha);
  return [
    Math.round(26 + 18 * k),
    Math.round(26 + 18 * k),
    Math.round(25 + 17 * k),
  ];
}

export function flashAlpha(alpha: number): number {
  return Math.max(FLASH_FLOOR, alpha * FLASH_FACTOR);
}

function rgba(r: number, g: number, b: number, a: number): string {
  const alpha = Math.round(clamp(a, 0, 1) * 1000) / 1000;
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

export interface ThemeInput {
  panelAlpha: number;
  bubbleAlpha: number;
  compensation: boolean;
}

/**
 * 计算一整套主题 CSS 变量。
 * 面板层与气泡层**各自独立、互不联动**（§3.7 三层模型）。
 */
export function themeVars(input: ThemeInput): Record<string, string> {
  const { panelAlpha, bubbleAlpha, compensation } = input;
  const [r, g, b] = panelRgb(panelAlpha, compensation);
  const flash = flashAlpha(panelAlpha);

  // 气泡 alpha 过低时必须靠描边维持边界，否则相邻气泡会糊成一片（§3.7 边界约束）
  const otherStroke = Math.max(bubbleAlpha * 0.42, 0.13);
  const ownStroke = Math.max(bubbleAlpha * 0.9, 0.18);

  return {
    '--panel-a': String(panelAlpha),
    '--panel-a-flash': String(Math.round(flash * 1000) / 1000),
    '--panel-r': String(r),
    '--panel-g': String(g),
    '--panel-b': String(b),
    // 完整颜色串，CSS 侧禁止再套 calc()（踩坑 #1）
    '--panel-bg': rgba(r, g, b, panelAlpha),
    '--panel-bg-flash': rgba(r, g, b, flash),
    '--overflow-bg': rgba(28, 28, 27, 0.95),

    '--bub-a': String(bubbleAlpha),
    '--bub-other-bg': rgba(255, 255, 255, bubbleAlpha * 0.22),
    '--bub-other-stroke': rgba(255, 255, 255, otherStroke),
    '--bub-own-bg': rgba(55, 138, 221, bubbleAlpha),
    '--bub-own-stroke': rgba(133, 183, 235, ownStroke),
    '--bub-atme-stroke': rgba(239, 159, 39, bubbleAlpha * 1.4),
    '--input-bg': rgba(255, 255, 255, bubbleAlpha * 0.16),
  };
}

/** 把变量写进 :root。单独拆出来，方便测试只测 themeVars 的纯计算部分。 */
export function applyThemeVars(vars: Record<string, string>, root: HTMLElement): void {
  for (const [k, v] of Object.entries(vars)) root.style.setProperty(k, v);
}
