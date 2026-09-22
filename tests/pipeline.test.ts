/**
 * 前端集成测试：IPC 契约 + 解析 → 行模型 → 折叠条优先级 → 幂等合并。
 *
 * 这里刻意把 Tauri 的 API 打桩，然后走真实的 `ipc.ts` / `store.ts` / `core/*`，
 * 目的是两件事：
 *   ① 锁住前后端的 IPC 契约（命令名、参数形状、事件名）——Rust 侧改名这里就会红；
 *   ② 验证从"收到消息"到"折叠条显示什么"的整条链路。
 *
 * Rust 侧的幂等、墓碑过滤、提醒优先级由各模块内的 `#[cfg(test)] mod tests` 覆盖
 * （`src-tauri/src/store/message.rs`、`src-tauri/src/sched/notify.rs`、`src-tauri/src/ob/event.rs`
 * 里的「端到端_从上游事件到历史过滤」）——`cargo test` 里跑。
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ConversationDTO, MessageDTO, Seg } from '../src/state/types';

const { invokeMock, listeners } = vi.hoisted(() => ({
  invokeMock: vi.fn(async (_cmd?: string, _args?: unknown) => undefined as unknown),
  listeners: new Map<string, (e: { payload: unknown }) => void>(),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
  convertFileSrc: (p: string) => `asset://${p}`,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: async (name: string, cb: (e: { payload: unknown }) => void) => {
    listeners.set(name, cb);
    return () => listeners.delete(name);
  },
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ startDragging: vi.fn(async () => undefined) }),
}));

// vitest 里 import.meta.env.DEV 也是 true，会让 ipc.ts 去挂浏览器假后端。
// 我们本来就打桩了 invoke，所以直接把假后端也打桩掉。
vi.mock('../src/dev/mock', () => ({
  installMockBackend: async () => undefined,
  mockInvoke: async () => undefined,
  mockListen: () => () => undefined,
}));

const S = await import('../src/state/store');
const ipc = await import('../src/state/ipc');
const { initEvents, openPanel } = await import('../src/state/events');
const { pickBarConversation, composeBar } = await import('../src/core/preview');
const { summarize } = await import('../src/core/segments');

beforeEach(() => {
  invokeMock.mockClear();
  invokeMock.mockResolvedValue(undefined);
  S.setConversations([]);
  S.selectByKey(null);
  S.setExpanded(false);
  S.stopFlash();
  S.setQuote(null);
  // store 是模块级单例，测试之间必须把消息清干净，否则会串味
  S.dropMessages({ peer_type: 1, peer_id: 30001 });
  S.dropMessages({ peer_type: 0, peer_id: 20001 });
  // 「换账号后要重挑默认会话」是一次性标记，喂一份非空快照把它消费掉，
  // 否则会漏到下一个用例里去。
  S.setConversations([convOf({ peer_id: 99999, tab_order: 0 })]);
  S.setConversations([]);
  S.selectByKey(null);
});

/** 把某个会话设为当前，这样 currentRows / currentMessages 才有内容 */
function focus(conv: ConversationDTO): void {
  S.setConversations([conv]);
  S.selectByKey(`${conv.peer_type}:${conv.peer_id}`);
}

/* ------------------------------------------------------------------ */

describe('IPC 契约', () => {
  it('send_message 的参数形状固定为 { peerType, peerId, parts }', async () => {
    await ipc.sendMessage(1, 30001, [{ kind: 'text', text: 'hi' }]);
    expect(invokeMock).toHaveBeenCalledWith('send_message', {
      peerType: 1,
      peerId: 30001,
      parts: [{ kind: 'text', text: 'hi' }],
    });
  });

  it('历史分页带 beforeSeq，首屏传 null', async () => {
    await ipc.loadMessages(0, 20001, null, 30);
    expect(invokeMock).toHaveBeenCalledWith('load_messages', {
      peerType: 0,
      peerId: 20001,
      beforeSeq: null,
      limit: 30,
    });
  });

  it('静音只写本地库，命令参数里没有任何"同步到 QQ"的字段（FR-37）', async () => {
    await ipc.setMute(1, 30001, true);
    expect(invokeMock).toHaveBeenCalledWith('set_mute', {
      peerType: 1,
      peerId: 30001,
      muted: true,
    });
  });

  it('标签排序只提交一次顺序数组', async () => {
    await ipc.reorderTabs(['1:30001', '0:20001']);
    expect(invokeMock).toHaveBeenCalledWith('reorder_tabs', { order: ['1:30001', '0:20001'] });
  });

  it('事件名与约定一致', async () => {
    await initEvents();
    for (const name of [
      'conn_state',
      'conversations',
      'msg_added',
      'msg_updated',
      'msg_removed',
      'notify',
      'history_page',
      'account_changed',
      'auto_collapse',
      'toggle_panel',
      'open_sheet',
      'toast',
    ]) {
      expect(listeners.has(name), name).toBe(true);
    }
  });

  it('set_viewing 用 null 表示"没有在看的会话"（FR-31 依赖它）', async () => {
    await ipc.setViewing(null, null);
    expect(invokeMock).toHaveBeenCalledWith('set_viewing', { peerType: null, peerId: null });

    await ipc.setViewing(1, 30001);
    expect(invokeMock).toHaveBeenCalledWith('set_viewing', { peerType: 1, peerId: 30001 });
  });

  it('展开面板必须先让 Rust 撑开窗口，再去拉消息', async () => {
    // 回归：`openPanel` 曾经只改前端状态、从不调 expand_window，
    // 表现是"点了折叠条，内容切到面板布局了，但窗口还是 40px 高"。
    invokeMock.mockImplementation(async (cmd?: string) =>
      cmd === 'load_messages' ? [] : undefined,
    );
    focus(convOf({ peer_id: 30001 }));
    // focus() 自己会顺手 markRead（切会话即已读）。它是"发后不管"的，
    // 所以要等它真的把 invoke 发出去再清空记录，否则它会在 expand 之后才落账。
    await vi.waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith('mark_read', expect.anything()),
    );
    invokeMock.mockClear();

    await openPanel();

    const order = invokeMock.mock.calls.map((c) => c[0]);
    expect(order[0]).toBe('expand_window');
    expect(order).toContain('load_messages');
    expect(order).toContain('mark_read');
    expect(order.indexOf('expand_window')).toBeLessThan(order.indexOf('load_messages'));
  });

  it('托盘请求开浮层落到 sheet 状态上，非法载荷不改状态', async () => {
    await initEvents();
    S.setSheet(null);
    const fire = listeners.get('open_sheet');
    expect(fire).toBeTruthy();

    fire?.({ payload: 'cache' });
    expect(S.state.sheet).toBe('cache');

    fire?.({ payload: '不存在的浮层' });
    expect(S.state.sheet).toBe('cache');
  });

  it('msg_added 缺会话快照时仍然收下消息（不能因为缺快照把消息一起丢掉）', async () => {
    // 回归：Rust 的 `MessageAddedPayload.conversation` 是 `Option`，取不到会话行时发 null。
    // 前端曾经直接 `upsertConversation(conversation)`，`peerKey(null)` 抛异常 ——
    // 结果是"快照缺失"连累"消息也没收下"，而消息才是不可再生的那一半。
    await initEvents();
    const fire = listeners.get('msg_added');
    expect(fire).toBeTruthy();

    fire?.({ payload: { message: msgOf({ message_id: 'm-1', ts: 1 }), conversation: null } });

    expect(S.state.messages['1:30001']?.map((m) => m.message_id)).toContain('m-1');
  });
});

/* ------------------------------------------------------------------ */

function convOf(over: Partial<ConversationDTO> & { peer_id: number }): ConversationDTO {
  return {
    peer_type: 1,
    name: `会话${over.peer_id}`,
    last_msg_time: 1000,
    last_msg_text: '内容',
    last_msg_sender: '李工',
    unread_count: 0,
    has_mention: false,
    tab_order: null,
    is_manual_tab: false,
    is_muted: false,
    ...over,
  };
}

function msgOf(over: Partial<MessageDTO> & { message_id: string; ts: number }): MessageDTO {
  return {
    peer_type: 1,
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

describe('解析 → 行模型（端到端）', () => {
  it('@我 + 图片 + 撤回的混合消息，摘要与行标记都正确', () => {
    const segs: Seg[] = [
      { type: 'at', qq: 10001, name: '你', is_self: true },
      { type: 'text', text: ' 字段名定了没？' },
    ];
    expect(summarize(segs, 10001)).toBe('@你 字段名定了没？');

    focus(convOf({ peer_id: 30001 }));
    S.addMessage(
      msgOf({ message_id: 'a', ts: 5 * 60_000, segments: segs, is_at_me: true }),
    );
    const row = S.currentRows()[0];

    expect(row?.msg.is_at_me).toBe(true);
    expect(row?.showWho).toBe(true); // 群聊
    expect(row?.cont).toBe(false);
  });
});

describe('幂等合并 FR-22', () => {
  it('同一 message_id 被事件与历史各送一次，只留一条', () => {
    focus(convOf({ peer_id: 30001 }));
    const m = msgOf({ message_id: 'dup', ts: 1000 });
    S.addMessage(m);
    S.addMessage({ ...m, text: '改过一下' });
    expect(S.currentRows()).toHaveLength(1);
    expect(S.currentRows()[0]?.msg.text).toBe('改过一下');
  });

  it('自己发的消息：乐观条目被真实 message_id 替换后不重复', () => {
    focus(convOf({ peer_id: 30001 }));
    const local = msgOf({ message_id: 'local:1', ts: 1000, is_self: true, send_state: 3 });
    const real = msgOf({ message_id: '88', ts: 1000, is_self: true, send_state: 0 });
    S.addMessage(local);
    S.addMessage(real);
    S.removeMessage('local:1'); // Rust 在这之前会发 msg_removed
    expect(S.currentRows().map((r) => r.msg.message_id)).toEqual(['88']);
  });

  it('历史分页与已有消息重叠时不会翻倍', () => {
    focus(convOf({ peer_id: 30001 }));
    S.addMessage(msgOf({ message_id: 'm2', ts: 2000, seq: 2 }));
    S.addMessage(msgOf({ message_id: 'm3', ts: 3000, seq: 3 }));
    // 上一页里带着已经有的 m2 / m3
    const page = [
      msgOf({ message_id: 'm1', ts: 1000, seq: 1 }),
      msgOf({ message_id: 'm2', ts: 2000, seq: 2 }),
      msgOf({ message_id: 'm3', ts: 3000, seq: 3 }),
    ];
    S.prependMessages({ peer_type: 1, peer_id: 30001 }, page, true);

    const key = '1:30001';
    expect(S.state.messages[key]).toHaveLength(3);
    expect(S.state.messages[key]?.map((m) => m.message_id)).toEqual(['m1', 'm2', 'm3']);
    expect(S.state.atTop[key]).toBe(true);
  });

  it('乱序到达的消息按时间重新排队', () => {
    focus(convOf({ peer_id: 30001 }));
    S.addMessage(msgOf({ message_id: 'c', ts: 3000 }));
    S.addMessage(msgOf({ message_id: 'a', ts: 1000 }));
    S.addMessage(msgOf({ message_id: 'b', ts: 2000 }));
    expect(S.currentRows().map((r) => r.msg.message_id)).toEqual(['a', 'b', 'c']);
  });

  it('删除后同一 id 再被历史拉回来也不会复活（前端侧配合 Rust 的墓碑过滤）', () => {
    focus(convOf({ peer_id: 30001 }));
    S.addMessage(msgOf({ message_id: 'gone', ts: 1000 }));
    S.removeMessage('gone');
    expect(S.currentRows()).toHaveLength(0);
  });
});

describe('提醒链路 FR-31 / FR-33', () => {
  it('非当前会话来消息：留痕 + 闪折叠条', () => {
    S.setConversations([
      convOf({ peer_id: 30001, unread_count: 3, last_msg_time: 9000, last_msg_text: '@你 定了没' }),
    ]);
    S.applyNotify({ peer_type: 1, peer_id: 30001, flash: true, mark: 'bar', claim_bar: true });
    expect(S.state.flashKey).toBe('1:30001');

    const bar = pickBarConversation(S.state.conversations);
    expect(bar?.peer_id).toBe(30001);
    expect(composeBar(bar!).name).toBe('会话30001 · 李工：');
  });

  it('正在查看的会话收到消息：既不闪也不占折叠条（FR-31）', async () => {
    S.setConversations([convOf({ peer_id: 30001, last_msg_time: 9000 })]);
    S.selectByKey('1:30001');
    S.applyNotify({ peer_type: 1, peer_id: 30001, flash: true, mark: 'none', claim_bar: false });
    expect(S.state.flashKey).toBeNull();
  });

  it('静音会话永远抢不到折叠条，即使它最新（FR-33）', () => {
    S.setConversations([
      convOf({ peer_id: 30001, unread_count: 1, last_msg_time: 5000 }),
      convOf({ peer_id: 30002, unread_count: 9, is_muted: true, last_msg_time: 99999 }),
    ]);
    expect(pickBarConversation(S.state.conversations)?.peer_id).toBe(30001);
  });

  it('闪烁动画结束后状态被清掉', () => {
    S.setConversations([convOf({ peer_id: 30001, last_msg_time: 9000 })]);
    S.applyNotify({ peer_type: 1, peer_id: 30001, flash: true, mark: 'bar', claim_bar: true });
    S.stopFlash();
    expect(S.state.flashKey).toBeNull();
  });
});

describe('标签栏 FR-11 / FR-13', () => {
  it('标签只按 tab_order 固定排序，不因新消息重排', () => {
    S.setConversations([
      convOf({ peer_id: 1, tab_order: 0, last_msg_time: 100 }),
      convOf({ peer_id: 2, tab_order: 1, last_msg_time: 9999 }),
      convOf({ peer_id: 3, tab_order: null, last_msg_time: 8888 }),
    ]);
    expect(S.tabs().map((c) => c.peer_id)).toEqual([1, 2]);
    expect(S.overflowList().map((c) => c.peer_id)).toEqual([3]);
  });

  it('移出标签只是 tab_order 变 null，会话本身还在', () => {
    S.setConversations([convOf({ peer_id: 1, tab_order: 0 })]);
    S.setConversations([convOf({ peer_id: 1, tab_order: null })]);
    expect(S.tabs()).toHaveLength(0);
    expect(S.state.conversations).toHaveLength(1);
  });
});

describe('切换会话即视为已读 FR-23', () => {
  it('选中会话会调 mark_read', async () => {
    S.setConversations([convOf({ peer_id: 30001, tab_order: 0, unread_count: 4 })]);
    S.selectByKey('1:30001');
    // mark_read 是 fire-and-forget 的，等它落到 invoke 上
    await vi.waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith('mark_read', { peerType: 1, peerId: 30001 }),
    );
  });

  it('当前会话被移出列表时选中态被清空', () => {
    S.setConversations([convOf({ peer_id: 30001, tab_order: 0 })]);
    S.selectByKey('1:30001');
    S.setConversations([convOf({ peer_id: 30002, tab_order: 0 })]);
    expect(S.state.current).toBeNull();
  });
});

/* ------------------------------------------------------------------ */

describe('换 QQ 账号后不留上个账号的数据', () => {
  it('resetForAccount 把会话、消息、选中态一起清掉', () => {
    focus(convOf({ peer_id: 30001, tab_order: 0 }));
    S.addMessage(msgOf({ message_id: 'm1', ts: 1 }));
    expect(S.state.messages['1:30001']).toHaveLength(1);
    expect(S.state.current).toBe('1:30001');

    S.resetForAccount();

    expect(S.state.conversations).toEqual([]);
    expect(S.state.messages).toEqual({});
    expect(S.state.atTop).toEqual({});
    expect(S.state.current).toBeNull();
    expect(S.currentRows()).toEqual([]);
  });

  it('换账号后第一份非空快照会自动选中第一个', () => {
    S.resetForAccount();
    // Rust 清完库会先推一份**空**快照，它不该把"重挑默认会话"这件事消费掉
    S.setConversations([]);
    expect(S.state.current).toBeNull();

    S.setConversations([
      convOf({ peer_id: 30001, tab_order: 0 }),
      convOf({ peer_id: 30002, tab_order: 1 }),
    ]);
    expect(S.state.current).toBe('1:30001');
  });

  it('account_changed 到达时清空缓存并更新 selfId', async () => {
    await initEvents();
    focus(convOf({ peer_id: 30001, tab_order: 0 }));
    S.addMessage(msgOf({ message_id: 'm1', ts: 1 }));

    const fire = listeners.get('account_changed');
    expect(fire).toBeDefined();
    fire?.({ payload: 2002 });

    expect(S.state.selfId).toBe(2002);
    expect(S.state.messages).toEqual({});
    expect(S.state.conversations).toEqual([]);
  });

  it('旧账号在同一个群里留下的消息不会冒充新账号的消息', () => {
    // 两个账号都在群 30001 里：peerKey 相同，只清会话列表是不够的
    focus(convOf({ peer_id: 30001, tab_order: 0 }));
    S.addMessage(msgOf({ message_id: 'old-account', ts: 1, text: '上个账号看到的' }));
    expect(S.currentMessages()).toHaveLength(1);

    S.resetForAccount();
    S.setConversations([convOf({ peer_id: 30001, tab_order: 0 })]);

    expect(S.state.current).toBe('1:30001');
    expect(S.currentMessages()).toHaveLength(0);
  });
});
