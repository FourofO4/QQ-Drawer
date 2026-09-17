/**
 * 消息段的展示侧处理（§4.7 接收/渲染部分）。
 *
 * 注意：这里**不做 OneBot 协议解析**——协议的一切形态终结在 Rust 的 `ob` 层（NFR-13）。
 * 本模块只处理已经规范化过的 `Seg[]`：合成单行摘要、取纯文本、给占位文案。
 */

import type { Seg } from '../state/types';

/** 占位文案（§3.10 文案表，逐条对齐，不要自由发挥） */
export const PLACEHOLDER_TEXT = {
  face: '[表情]',
  mface: '[表情]',
  record: '[语音]',
  video: '[视频]',
  file: '[文件]',
  card: '[卡片消息]',
  forward: '[合并转发]',
  unknown: '[暂不支持的消息类型]',
} as const;

export const IMAGE_CLEANED = '[图片已清除]';
export const IMAGE_FAILED = '[图片加载失败]';
export const IMAGE_BLOCK = '[图片]';

/** 段 → 一行摘要里该出现的文本。图片/表情一律用占位文案，不展开内容。 */
export function segSummary(seg: Seg, selfId: number | null): string {
  switch (seg.type) {
    case 'text':
      return seg.text;
    case 'image':
      if (seg.state === 4) return IMAGE_CLEANED;
      if (seg.state === 3) return IMAGE_FAILED;
      return IMAGE_BLOCK;
    case 'at':
      if (seg.is_self || (selfId !== null && seg.qq === selfId)) return '@你';
      return `@${seg.name ?? seg.qq}`;
    case 'reply':
      // 引用段本身不出现在摘要里，由 reply_preview 单独渲染
      return '';
    case 'placeholder':
      return seg.text || PLACEHOLDER_TEXT[seg.kind];
    case 'system':
      return seg.text;
    default:
      return '';
  }
}

/**
 * 单行摘要：折叠条预览、`···` 列表、引用块都用它。
 * 换行、制表、连续空白全部压成单个空格——折叠条必须是单行（FR-02）。
 */
export function summarize(segs: readonly Seg[], selfId: number | null = null): string {
  const raw = segs.map((s) => segSummary(s, selfId)).join('');
  return flatten(raw);
}

/** 压成单行：折叠条与列表项都是单行展示，任何换行都会破坏布局 */
export function flatten(text: string): string {
  return text.replace(/[\r\n\t\u2028\u2029]+/g, ' ').replace(/\s{2,}/g, ' ').trim();
}

/** 纯文本合并结果——「复制文本」用（FR-21 / FR-49） */
export function plainText(segs: readonly Seg[]): string {
  const parts: string[] = [];
  for (const seg of segs) {
    switch (seg.type) {
      case 'text':
        parts.push(seg.text);
        break;
      case 'at':
        parts.push(`@${seg.name ?? seg.qq}`);
        break;
      case 'image':
        parts.push(seg.state === 4 ? IMAGE_CLEANED : IMAGE_BLOCK);
        break;
      case 'placeholder':
        parts.push(seg.text || PLACEHOLDER_TEXT[seg.kind]);
        break;
      case 'system':
        parts.push(seg.text);
        break;
      default:
        break;
    }
  }
  return parts.join('');
}

/** 该消息是否含有需要落盘渲染的图片 */
export function imageSegments(segs: readonly Seg[]): Extract<Seg, { type: 'image' }>[] {
  return segs.filter((s): s is Extract<Seg, { type: 'image' }> => s.type === 'image');
}

/**
 * 气泡是否需要描边维持边界。
 * §3.7 边界约束：气泡 alpha 低于 0.2 时**必须**靠 0.5px 描边分界，否则相邻气泡糊成一片。
 */
export function needsBubbleStroke(bubbleAlpha: number): boolean {
  return bubbleAlpha < 0.2;
}
