import { describe, expect, it } from 'vitest';
import {
  appendText,
  blockReasonText,
  canSend,
  filterMembers,
  hasAt,
  hasAtUser,
  insertAt,
  insertText,
  isEmpty,
  normalize,
  partLabel,
  plainText,
  quoteOf,
  removePart,
  type Part,
} from './composer-model';

const atPart: Part = { kind: 'at', qq: 30011, name: '李工' };
const imgPart: Part = { kind: 'image', sha256: 'abc', path: 'C:/tmp/a.png' };

describe('令牌模型 FR-28', () => {
  it('相邻文本片段被合并，空片段被丢掉', () => {
    const parts: Part[] = [
      { kind: 'text', text: 'a' },
      { kind: 'text', text: '' },
      { kind: 'text', text: 'b' },
    ];
    expect(normalize(parts)).toEqual([{ kind: 'text', text: 'ab' }]);
  });

  it('令牌整体删除，不会破坏相邻文本', () => {
    const parts: Part[] = [{ kind: 'text', text: '你好 ' }, atPart, { kind: 'text', text: ' 收到' }];
    expect(removePart(parts, 1)).toEqual([
      { kind: 'text', text: '你好  收到' },
    ]);
  });

  it('插入位置越界时被夹到两端', () => {
    const parts: Part[] = [{ kind: 'text', text: 'b' }];
    expect(insertText(parts, -5, 'a')).toEqual([{ kind: 'text', text: 'ab' }]);
    expect(insertText(parts, 99, 'c')).toEqual([{ kind: 'text', text: 'bc' }]);
  });

  it('插入 @ 令牌不参与文本合并', () => {
    const parts = insertAt([], 0, atPart);
    expect(parts).toEqual([atPart]);
  });

  it('令牌展示为纯文字，不做缩略图预览（FR-26）', () => {
    expect(partLabel(imgPart)).toBe('[图片]');
    expect(partLabel(atPart)).toBe('@李工');
  });

  it('纯文本拼接用于摘要', () => {
    expect(plainText([{ kind: 'text', text: '看这个' }, imgPart])).toBe('看这个[图片]');
  });

  it('@ 重复插入可被检测', () => {
    const parts: Part[] = [atPart];
    expect(hasAtUser(parts, 30011)).toBe(true);
    expect(hasAtUser(parts, 30012)).toBe(false);
    expect(hasAt(parts)).toBe(true);
  });
});

describe('发送前校验 §4.7', () => {
  it('空内容不发送', () => {
    expect(canSend([], 1)).toEqual({ ok: false, reason: 'empty' });
  });

  it('纯空白不发送', () => {
    expect(canSend([{ kind: 'text', text: '   \n  ' }], 1)).toEqual({
      ok: false,
      reason: 'empty',
    });
    expect(isEmpty([{ kind: 'text', text: ' \t ' }])).toBe(true);
  });

  it('只有一块令牌、没有文本时按图片消息正常发送', () => {
    expect(canSend([imgPart], 1).ok).toBe(true);
    expect(canSend([atPart], 1).ok).toBe(true);
  });

  it('私聊不允许 at 令牌', () => {
    expect(canSend([atPart], 0)).toEqual({ ok: false, reason: 'at_in_private' });
  });

  it('只挂了引用、正文空白时不允许发送', () => {
    const parts: Part[] = [{ kind: 'reply', id: '9' }, { kind: 'text', text: '  ' }];
    expect(canSend(parts, 1).ok).toBe(false);
  });

  it('引用 + 正文可以发送，且引用 id 可被取出', () => {
    const parts: Part[] = [{ kind: 'reply', id: '9' }, { kind: 'text', text: '收到' }];
    expect(canSend(parts, 1).ok).toBe(true);
    expect(quoteOf(parts)).toBe('9');
  });

  it('拦截原因有对应文案', () => {
    expect(blockReasonText('empty')).toBe('没有可发送的内容');
    expect(blockReasonText('at_in_private')).toBe('私聊里不能 @ 人');
  });
});

describe('@ 选人过滤 FR-27', () => {
  const members = [
    { user_id: 30011, display_name: '李工' },
    { user_id: 30012, display_name: '小陈' },
    { user_id: 30013, display_name: '王工' },
  ];

  it('空关键词返回全部（截断到上限）', () => {
    expect(filterMembers(members, '')).toHaveLength(3);
    expect(filterMembers(members, '', 2)).toHaveLength(2);
  });

  it('按名字子串过滤', () => {
    expect(filterMembers(members, '工').map((m) => m.user_id)).toEqual([30011, 30013]);
  });

  it('按 QQ 号过滤', () => {
    expect(filterMembers(members, '30012').map((m) => m.display_name)).toEqual(['小陈']);
  });

  it('没有匹配时不返回内容', () => {
    expect(filterMembers(members, '不存在的人')).toEqual([]);
  });
});

describe('追加与清空', () => {
  it('appendText 追加到末尾', () => {
    expect(appendText([{ kind: 'text', text: 'a' }], 'b')).toEqual([{ kind: 'text', text: 'ab' }]);
  });

  it('追加空串无副作用', () => {
    const parts: Part[] = [{ kind: 'text', text: 'a' }];
    expect(appendText(parts, '')).toEqual(parts);
  });
});
