import { describe, expect, it } from 'vitest';
import type { ConversationDTO } from '../state/types';
import {
  BAR_TEXT,
  barHasMark,
  barNamePrefix,
  composeBar,
  itemHasMark,
  peerKey,
  pickBarConversation,
} from './preview';

function conv(over: Partial<ConversationDTO>): ConversationDTO {
  return {
    peer_type: 0,
    peer_id: 1,
    name: '某人',
    last_msg_time: 1000,
    last_msg_text: '内容',
    last_msg_sender: null,
    unread_count: 0,
    has_mention: false,
    tab_order: null,
    is_manual_tab: false,
    is_muted: false,
    ...over,
  };
}

describe('折叠条优先级 FR-33', () => {
  it('没有会话时返回 null', () => {
    expect(pickBarConversation([])).toBeNull();
  });

  it('还没有任何消息的会话不参与竞争', () => {
    const c = conv({ last_msg_time: null });
    expect(pickBarConversation([c])).toBeNull();
  });

  it('① 优先抢未读的非静音会话，且取最新的一条', () => {
    const older = conv({ peer_id: 1, unread_count: 1, last_msg_time: 5000 });
    const newer = conv({ peer_id: 2, unread_count: 1, last_msg_time: 9000 });
    const mutedNewest = conv({ peer_id: 3, unread_count: 5, is_muted: true, last_msg_time: 9999 });
    expect(pickBarConversation([older, newer, mutedNewest])?.peer_id).toBe(2);
  });

  it('静音会话永远抢不到折叠条位置（只要存在未读的非静音会话）', () => {
    const muted = conv({ peer_id: 3, unread_count: 9, is_muted: true, last_msg_time: 99999 });
    const hot = conv({ peer_id: 4, unread_count: 1, last_msg_time: 1 });
    expect(pickBarConversation([muted, hot])?.peer_id).toBe(4);
  });

  it('② 没有非静音未读时，让静音未读会话上折叠条', () => {
    const muted = conv({ peer_id: 3, unread_count: 2, is_muted: true, last_msg_time: 7000 });
    const read = conv({ peer_id: 4, unread_count: 0, last_msg_time: 9000 });
    expect(pickBarConversation([muted, read])?.peer_id).toBe(3);
  });

  it('③ 完全没有未读时，显示全局最新的一条', () => {
    const a = conv({ peer_id: 1, last_msg_time: 7000 });
    const b = conv({ peer_id: 2, last_msg_time: 8000 });
    expect(pickBarConversation([a, b])?.peer_id).toBe(2);
  });
});

describe('折叠条文本 §3.2', () => {
  it('私聊只有会话名，不带发送者', () => {
    const c = conv({ name: '小陈', last_msg_text: '今天几点到？', last_msg_sender: '小陈' });
    expect(composeBar(c)).toEqual({ name: '小陈：', body: '今天几点到？' });
  });

  it('群聊带上发送者：群名 · 老王：', () => {
    const c = conv({
      peer_type: 1,
      name: '产品组',
      last_msg_sender: '老王',
      last_msg_text: '这份原型下周再评审吧',
    });
    expect(barNamePrefix(c)).toBe('产品组 · 老王');
    expect(composeBar(c).name).toBe('产品组 · 老王：');
  });

  it('群名与发送者同名时不重复拼接', () => {
    const c = conv({ peer_type: 1, name: '读书会', last_msg_sender: '读书会' });
    expect(barNamePrefix(c)).toBe('读书会');
  });

  it('内容换行被压成单行——折叠条必须是单行', () => {
    const c = conv({ name: '小陈', last_msg_text: '第一行\n第二行\t第三行' });
    expect(composeBar(c).body).toBe('第一行 第二行 第三行');
  });

  it('没有最后一条消息时内容为空串', () => {
    const c = conv({ last_msg_text: null });
    expect(composeBar(c).body).toBe('');
  });

  it('固定文案与规范一致', () => {
    expect(BAR_TEXT.connecting).toBe('连接中…');
    expect(BAR_TEXT.disconnected).toBe('连接已断开 · 点击重试');
    expect(BAR_TEXT.empty).toBe('暂无会话');
  });
});

describe('痕迹判定 FR-34', () => {
  it('未读且非静音 → 留痕', () => {
    expect(barHasMark(conv({ unread_count: 1 }), null)).toBe(true);
  });

  it('静音不留痕（静音是一个承诺）', () => {
    expect(barHasMark(conv({ unread_count: 3, is_muted: true }), null)).toBe(false);
  });

  it('正在查看的会话不留痕', () => {
    const c = conv({ peer_type: 1, peer_id: 30001, unread_count: 2 });
    expect(barHasMark(c, peerKey(c))).toBe(false);
  });

  it('标签 / ··· 项的痕迹口径与折叠条一致', () => {
    const c = conv({ unread_count: 2 });
    expect(itemHasMark(c, false, false)).toBe(true);
    expect(itemHasMark(c, true, false)).toBe(false);
    expect(itemHasMark(c, false, true)).toBe(false);
  });
});
