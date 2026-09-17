import { describe, expect, it } from 'vitest';
import type { Seg } from '../state/types';
import {
  IMAGE_BLOCK,
  IMAGE_CLEANED,
  IMAGE_FAILED,
  PLACEHOLDER_TEXT,
  flatten,
  imageSegments,
  needsBubbleStroke,
  plainText,
  segSummary,
  summarize,
} from './segments';

describe('消息段摘要 §4.7', () => {
  it('文本直出', () => {
    expect(segSummary({ type: 'text', text: '你好' }, 1)).toBe('你好');
  });

  it('图片在摘要里是占位，不展开', () => {
    expect(segSummary({ type: 'image', path: '/a.png', sub_type: 0, state: 1 }, 1)).toBe(
      IMAGE_BLOCK,
    );
  });

  it('已清理的图片显示 [图片已清除]', () => {
    expect(segSummary({ type: 'image', path: null, sub_type: 0, state: 4 }, 1)).toBe(
      IMAGE_CLEANED,
    );
  });

  it('落盘失败的图片显示 [图片加载失败]', () => {
    expect(segSummary({ type: 'image', path: null, sub_type: 0, state: 3 }, 1)).toBe(
      IMAGE_FAILED,
    );
  });

  it('@自己显示为 @你', () => {
    expect(segSummary({ type: 'at', qq: 7, name: '我', is_self: true }, 7)).toBe('@你');
  });

  it('@别人用名字，没有名字时退回 qq', () => {
    expect(segSummary({ type: 'at', qq: 8, name: '李工', is_self: false }, 7)).toBe('@李工');
    expect(segSummary({ type: 'at', qq: 8, name: null, is_self: false }, 7)).toBe('@8');
  });

  it('引用段本身不进摘要（由引用块单独渲染）', () => {
    expect(segSummary({ type: 'reply', id: '1' }, 1)).toBe('');
  });

  it('各类占位文案与文案表一致', () => {
    expect(segSummary({ type: 'placeholder', kind: 'face', text: '' }, 1)).toBe('[表情]');
    expect(segSummary({ type: 'placeholder', kind: 'record', text: '' }, 1)).toBe('[语音]');
    expect(segSummary({ type: 'placeholder', kind: 'video', text: '' }, 1)).toBe('[视频]');
    expect(segSummary({ type: 'placeholder', kind: 'file', text: '' }, 1)).toBe('[文件]');
    expect(segSummary({ type: 'placeholder', kind: 'card', text: '' }, 1)).toBe('[卡片消息]');
    expect(segSummary({ type: 'placeholder', kind: 'forward', text: '' }, 1)).toBe('[合并转发]');
    expect(segSummary({ type: 'placeholder', kind: 'unknown', text: '' }, 1)).toBe(
      '[暂不支持的消息类型]',
    );
    expect(PLACEHOLDER_TEXT.mface).toBe('[表情]');
  });
});

describe('多段合成', () => {
  const segs: Seg[] = [
    { type: 'at', qq: 7, name: '我', is_self: true },
    { type: 'text', text: ' 那个接口的字段名定了没？' },
    { type: 'image', path: '/a.png', sub_type: 0, state: 1 },
  ];

  it('按顺序拼接', () => {
    expect(summarize(segs, 7)).toBe('@你 那个接口的字段名定了没？[图片]');
  });

  it('引用段被跳过', () => {
    const withReply: Seg[] = [{ type: 'reply', id: '9' }, ...segs];
    expect(summarize(withReply, 7)).toBe('@你 那个接口的字段名定了没？[图片]');
  });

  it('纯文本结果用于复制（FR-21）', () => {
    expect(plainText(segs)).toBe('@我 那个接口的字段名定了没？[图片]');
  });
});

describe('单行化', () => {
  it('换行、制表、连续空白全部压成单个空格', () => {
    expect(flatten('a\nb\tc   d')).toBe('a b c d');
  });

  it('首尾空白被裁掉', () => {
    expect(flatten('  收工  ')).toBe('收工');
  });

  it('Unicode 行分隔符也要处理', () => {
    expect(flatten('a\u2028b\u2029c')).toBe('a b c');
  });
});

describe('图片段提取与描边约束', () => {
  it('只挑出 image 段', () => {
    const segs: Seg[] = [
      { type: 'text', text: 'x' },
      { type: 'image', path: '/a.png', sub_type: 0, state: 1 },
      { type: 'image', path: '/b.png', sub_type: 1, state: 1 },
    ];
    expect(imageSegments(segs)).toHaveLength(2);
  });

  it('气泡 alpha 低于 0.2 时必须依赖 0.5px 描边维持边界', () => {
    expect(needsBubbleStroke(0.35)).toBe(false);
    expect(needsBubbleStroke(0.199)).toBe(true);
    expect(needsBubbleStroke(0.08)).toBe(true);
  });
});
