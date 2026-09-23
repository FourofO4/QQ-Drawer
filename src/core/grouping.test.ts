import { describe, expect, it } from 'vitest';
import type { MessageDTO, PeerType } from '../state/types';
import { buildRows, createRowCache, oldestSeq } from './grouping';

const MIN = 60_000;
/** 固定一个"现在"，测试不依赖真实时间 */
const NOW = new Date(2026, 8, 17, 14, 0, 0).getTime();

function msg(over: Partial<MessageDTO> & { ts: number }): MessageDTO {
  return {
    message_id: `m${over.ts}`,
    peer_type: 1 as PeerType,
    peer_id: 30001,
    seq: 1,
    sender_id: 30011,
    sender_name: '李工',
    is_self: false,
    text: 'x',
    segments: [{ type: 'text', text: 'x' }],
    reply_to: null,
    reply_preview: null,
    is_at_me: false,
    has_image: false,
    image_state: 0,
    images: [],
    is_recalled: false,
    recalled_by: null,
    recalled_by_name: null,
    send_state: 0,
    ...over,
  };
}

describe('行模型与时间分隔 FR-19', () => {
  it('列表首条插入日期分隔，且带上时刻', () => {
    const rows = buildRows([msg({ ts: NOW })], NOW);
    expect(rows[0]?.separator).toEqual({ kind: 'day', text: '今天 14:00' });
  });

  it('间隔超过 5 分钟插入时刻分隔', () => {
    const rows = buildRows(
      [msg({ ts: NOW - 10 * MIN }), msg({ ts: NOW - 2 * MIN, message_id: 'b' })],
      NOW,
    );
    expect(rows[1]?.separator).toEqual({ kind: 'time', text: '13:58' });
  });

  it('间隔不超过 5 分钟不插入分隔', () => {
    const rows = buildRows(
      [msg({ ts: NOW - 3 * MIN }), msg({ ts: NOW - 1 * MIN, message_id: 'b' })],
      NOW,
    );
    expect(rows[1]?.separator).toBeNull();
  });

  it('跨天插入日期分隔', () => {
    const yesterday = new Date(2026, 8, 16, 22, 30, 0).getTime();
    const rows = buildRows([msg({ ts: yesterday }), msg({ ts: NOW, message_id: 'b' })], NOW);
    expect(rows[1]?.separator).toEqual({ kind: 'day', text: '今天 14:00' });
    expect(rows[0]?.separator).toEqual({ kind: 'day', text: '昨天 22:30' });
  });
});

describe('同一人连续发言合并 FR-17', () => {
  it('同一人短间隔连续发言：后续条目 cont', () => {
    const rows = buildRows(
      [
        msg({ ts: NOW - 4 * MIN }),
        msg({ ts: NOW - 3 * MIN, message_id: 'b' }),
        msg({ ts: NOW - 2 * MIN, message_id: 'c' }),
      ],
      NOW,
    );
    expect(rows.map((r) => r.cont)).toEqual([false, true, true]);
  });

  it('换人发言打断合并', () => {
    const rows = buildRows(
      [
        msg({ ts: NOW - 4 * MIN }),
        msg({ ts: NOW - 3 * MIN, message_id: 'b', sender_id: 30012, sender_name: '小陈' }),
      ],
      NOW,
    );
    expect(rows[1]?.cont).toBe(false);
  });

  it('中间插进时间分隔时不再合并', () => {
    const rows = buildRows(
      [msg({ ts: NOW - 20 * MIN }), msg({ ts: NOW - 1 * MIN, message_id: 'b' })],
      NOW,
    );
    expect(rows[1]?.cont).toBe(false);
  });

  it('自己发的与他人不合并', () => {
    const rows = buildRows(
      [
        msg({ ts: NOW - 4 * MIN }),
        msg({ ts: NOW - 3 * MIN, message_id: 'b', is_self: true, sender_id: 10001 }),
      ],
      NOW,
    );
    expect(rows[1]?.cont).toBe(false);
  });
});

describe('发送者标识 FR-17', () => {
  it('群聊首条标名带头像', () => {
    const rows = buildRows([msg({ ts: NOW })], NOW);
    expect(rows[0]?.showWho).toBe(true);
  });

  it('群聊续行不标名', () => {
    const rows = buildRows(
      [msg({ ts: NOW - 2 * MIN }), msg({ ts: NOW - 1 * MIN, message_id: 'b' })],
      NOW,
    );
    expect(rows[1]?.showWho).toBe(false);
  });

  it('私聊一律不标名（会话名即发送者）', () => {
    const rows = buildRows([msg({ ts: NOW, peer_type: 0 })], NOW);
    expect(rows[0]?.showWho).toBe(false);
  });

  it('自己发的一律不标名', () => {
    const rows = buildRows([msg({ ts: NOW, is_self: true, sender_id: 10001 })], NOW);
    expect(rows[0]?.showWho).toBe(false);
  });
});

describe('辅助函数', () => {
  it('取最早一条的 seq 作为向上翻页的游标', () => {
    const rows = buildRows(
      [msg({ ts: NOW - 2 * MIN, seq: 12 }), msg({ ts: NOW, message_id: 'b', seq: 13 })],
      NOW,
    );
    expect(oldestSeq(rows)).toBe(12);
  });

  it('空列表返回 null', () => {
    expect(oldestSeq([])).toBeNull();
  });

  it('行的 key 就是 message_id，虚拟滚动靠它做锚点', () => {
    const rows = buildRows([msg({ ts: NOW, message_id: 'abc' })], NOW);
    expect(rows[0]?.key).toBe('abc');
  });
});

/**
 * 这些用例钉的是一个**性能契约**，不是功能：`<For>` 按引用 diff，
 * 行对象一旦每次全新，一次消息追加就会把整个渲染窗口的 DOM 全部销毁重建，
 * 连带几十次行高重测与坐标系改写 —— 快速滚动时这个环会自激，滚轮就"不动了"。
 */
describe('行对象缓存：<For> 按引用 diff 的前提', () => {
  it('同样的输入重复构建，行对象引用保持不变', () => {
    const cache = createRowCache();
    const list = [msg({ ts: NOW - 2 * MIN }), msg({ ts: NOW - 1 * MIN, message_id: 'b' })];

    const first = cache.build(list, NOW);
    const second = cache.build(list, NOW);

    expect(second[0]).toBe(first[0]);
    expect(second[1]).toBe(first[1]);
  });

  it('向下追加：旧行原样复用，只有新行是新对象', () => {
    const cache = createRowCache();
    const a = msg({ ts: NOW - 2 * MIN });
    const b = msg({ ts: NOW - 1 * MIN, message_id: 'b' });

    const first = cache.build([a], NOW);
    const second = cache.build([a, b], NOW);

    expect(second[0]).toBe(first[0]);
    expect(second[1]).not.toBe(first[0]);
    expect(second[1]?.key).toBe('b');
  });

  it('向上翻页前插：只有被顶掉首行位置的那一行重建，其余原样复用', () => {
    const cache = createRowCache();
    const b = msg({ ts: NOW - 2 * MIN, message_id: 'b' });
    const c = msg({ ts: NOW - 1 * MIN, message_id: 'c' });
    const a = msg({ ts: NOW - 10 * MIN, message_id: 'a' });

    const first = cache.build([b, c], NOW);
    const second = cache.build([a, b, c], NOW);

    // b 原本是首行（分隔是「今天 13:58」），前插后降级成时刻分隔（「13:58」）→
    // 分隔文案确实变了，这一行**必须**重建，否则界面上会挂着过期的时间标签
    expect(second[1]).not.toBe(first[0]);
    expect(second[1]?.separator?.kind).toBe('time');
    // 非首行不受影响：一次翻页只重建 1 个节点，不是整窗
    expect(second[2]).toBe(first[1]);
  });

  it('消息内容变了就换新对象，DOM 才会跟着更新', () => {
    const cache = createRowCache();
    const before = msg({ ts: NOW, message_id: 'a', text: '原文' });
    const after = msg({ ts: NOW, message_id: 'a', text: '改过' });

    const first = cache.build([before], NOW);
    const second = cache.build([after], NOW);

    expect(second[0]).not.toBe(first[0]);
    expect(second[0]?.msg.text).toBe('改过');
  });

  it('前驱换人导致合并标记变化时，这一行也会重建', () => {
    const cache = createRowCache();
    const mine = msg({ ts: NOW - 2 * MIN, message_id: 'a', sender_id: 30011 });
    const other = msg({ ts: NOW - 1 * MIN, message_id: 'b', sender_id: 30012 });

    // 只有 a：单条，不合并
    const alone = cache.build([mine], NOW);
    expect(alone[0]?.cont).toBe(false);

    // a + b：a 仍不合并（它没有前驱），引用应该复用
    const withB = cache.build([mine, other], NOW);
    expect(withB[0]).toBe(alone[0]);
    expect(withB[1]?.cont, '换人了，b 不与 a 合并').toBe(false);
  });

  it('换会话后旧行从缓存里清掉，不会无界增长', () => {
    const cache = createRowCache();
    cache.build([msg({ ts: NOW, message_id: 'a' })], NOW);
    cache.build([msg({ ts: NOW, message_id: 'b' }), msg({ ts: NOW, message_id: 'c' })], NOW);

    expect(cache.size()).toBe(2);
  });

  it('clear 之后重新构建会产出新对象（换账号时用）', () => {
    const cache = createRowCache();
    const a = msg({ ts: NOW });

    const first = cache.build([a], NOW);
    cache.clear();

    expect(cache.size()).toBe(0);
    expect(cache.build([a], NOW)[0]).not.toBe(first[0]);
  });

  it('buildRows 仍是纯函数：两次调用给出不同的对象', () => {
    const a = msg({ ts: NOW });
    expect(buildRows([a], NOW)[0]).not.toBe(buildRows([a], NOW)[0]);
  });
});
