import { describe, expect, it } from 'vitest';
import type { MessageDTO, PeerType } from '../state/types';
import { buildRows, oldestSeq } from './grouping';

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
