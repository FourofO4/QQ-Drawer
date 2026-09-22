/**
 * 开发期假后端（**只在 `npm run dev` 且不在 Tauri 里时激活**）。
 *
 * 存在理由很实在：做界面的时候不该被 NapCat 和 Rust 构建拖着走，
 * 也不该在浏览器里看到一片空白。生产构建永远不会执行到这里
 * （`ipc.ts` 用 `import.meta.env.DEV` 把入口挡住了）。
 *
 * 这里的数据纯粹是为了把交互跑通，不含任何真实业务逻辑——
 * 提醒优先级、幂等、墓碑过滤这些判定一条都不在这里实现，它们在 Rust 侧。
 */

import type {
  CacheOverviewDTO,
  ConversationDTO,
  MemberDTO,
  MessageDTO,
  SettingsDTO,
} from '../state/types';

type Handler = (payload: unknown) => void;

const listeners: { event: string; handler: Handler }[] = [];

function emit(event: string, payload: unknown): void {
  for (const l of listeners) if (l.event === event) l.handler(payload);
}

export function mockListen<T>(event: string, handler: (payload: T) => void): () => void {
  const entry: { event: string; handler: Handler } = {
    event,
    handler: handler as unknown as Handler,
  };
  listeners.push(entry);
  return () => {
    const i = listeners.indexOf(entry);
    if (i >= 0) listeners.splice(i, 1);
  };
}

/* ------------------------------ 假数据 ------------------------------ */

const SELF_ID = 10001;
const NOW = Date.now();
const MIN = 60_000;

/** 内联 SVG 当占位图，浏览器里不依赖任何网络 */
function placeholderImage(label: string, w: number, h: number): string {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}">
    <rect width="100%" height="100%" fill="#3a4a6b"/>
    <circle cx="${w * 0.3}" cy="${h * 0.62}" r="${h * 0.22}" fill="#8fa8d8" opacity=".75"/>
    <path d="M0 ${h * 0.78} L${w * 0.42} ${h * 0.34} L${w * 0.7} ${h * 0.7} L${w} ${h * 0.42} L${w} ${h} L0 ${h} Z" fill="#6f8ab8" opacity=".8"/>
    <text x="8" y="16" font-family="sans-serif" font-size="11" fill="#e8e6df">${label}</text>
  </svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

const IMG = placeholderImage('图片', 296, 184);

function conv(
  peerType: 0 | 1,
  peerId: number,
  name: string,
  opts: Partial<ConversationDTO> = {},
): ConversationDTO {
  return {
    peer_type: peerType,
    peer_id: peerId,
    name,
    last_msg_time: NOW - 60 * MIN,
    last_msg_text: '在吗',
    last_msg_sender: null,
    unread_count: 0,
    has_mention: false,
    tab_order: null,
    is_manual_tab: false,
    is_muted: false,
    ...opts,
  };
}

const conversations: ConversationDTO[] = [
  conv(0, 20001, '小陈', {
    last_msg_time: NOW - 6 * MIN,
    last_msg_text: '你到了跟我说一声，我下去接你',
    last_msg_sender: '小陈',
    unread_count: 2,
    tab_order: 0,
  }),
  conv(1, 30001, '大前端交流群', {
    last_msg_time: NOW - 3 * MIN,
    last_msg_text: '@你 那个接口的字段名定了没？',
    last_msg_sender: '李工',
    unread_count: 1,
    has_mention: true,
    tab_order: 1,
  }),
  conv(1, 30002, '产品组', {
    last_msg_time: NOW - 42 * MIN,
    last_msg_text: '这份原型下周再评审吧',
    last_msg_sender: '老王',
    unread_count: 1,
    is_muted: true,
    tab_order: 2,
  }),
  conv(0, 20009, '老王', {
    last_msg_time: NOW - 12 * MIN,
    last_msg_text: '在吗？帮我看看这个报错',
    last_msg_sender: '老王',
    unread_count: 1,
  }),
  conv(0, 20010, '家电维修', {
    last_msg_time: NOW - 3 * 60 * MIN,
    last_msg_text: '好的，明天上午上门',
    last_msg_sender: '家电维修',
  }),
  conv(1, 30003, '室友群', {
    last_msg_time: NOW - 8 * 60 * MIN,
    last_msg_text: '周六爬山有人去吗',
    last_msg_sender: '大林',
  }),
  conv(0, 20011, '读书会', {
    last_msg_time: NOW - 26 * 60 * MIN,
    last_msg_text: '下周分享主题投票',
    last_msg_sender: '读书会',
  }),
];

const messages: Record<string, MessageDTO[]> = {};

function msg(
  id: string,
  peer: { peer_type: 0 | 1; peer_id: number },
  minutesAgo: number,
  senderId: number,
  senderName: string,
  segments: MessageDTO['segments'],
  extra: Partial<MessageDTO> = {},
): MessageDTO {
  return {
    message_id: id,
    peer_type: peer.peer_type,
    peer_id: peer.peer_id,
    seq: Number(id.replace(/\D/g, '')) || 1,
    ts: NOW - minutesAgo * MIN,
    sender_id: senderId,
    sender_name: senderName,
    is_self: senderId === SELF_ID,
    text: segments.map((s) => (s.type === 'text' ? s.text : '')).join(''),
    segments,
    reply_to: null,
    reply_preview: null,
    is_at_me: false,
    has_image: segments.some((s) => s.type === 'image'),
    image_state: 0,
    images: [],
    is_recalled: false,
    recalled_by: null,
    recalled_by_name: null,
    send_state: 0,
    ...extra,
  };
}

const group = { peer_type: 1 as const, peer_id: 30001 };
const priv = { peer_type: 0 as const, peer_id: 20001 };

messages['1:30001'] = [
  msg('90001', group, 96, 30011, '李工', [{ type: 'text', text: '早，昨天的联调结论同步一下' }]),
  msg('90002', group, 92, 30011, '李工', [
    { type: 'text', text: '先看这个' },
    { type: 'image', path: IMG, sub_type: 0, state: 1 },
  ]),
  msg('90003', group, 88, SELF_ID, '我', [{ type: 'text', text: '收到，下午对齐' }]),
  msg('90004', group, 62, 30012, '小陈', [{ type: 'text', text: '顺便把错误码表也带上' }]),
  msg(
    '90005',
    group,
    30,
    30011,
    '李工',
    [
      { type: 'at', qq: SELF_ID, name: '你', is_self: true },
      { type: 'text', text: ' 那个接口的字段名定了没？' },
    ],
    { is_at_me: true },
  ),
  msg(
    '90006',
    group,
    42,
    30013,
    '王姐',
    [{ type: 'text', text: '刚才发错了' }],
    { is_recalled: true, recalled_by: 30013, recalled_by_name: '王姐' },
  ),
  msg('90007', group, 3, 30011, '李工', [
    { type: 'text', text: '定了没？我这边要先冻结字段' },
  ]),
];

messages['0:20001'] = [
  msg('91001', priv, 24, 20001, '小陈', [{ type: 'text', text: '今天几点到？' }]),
  msg('91002', priv, 22, SELF_ID, '我', [{ type: 'text', text: '六点半落地' }]),
  msg('91003', priv, 6, 20001, '小陈', [{ type: 'text', text: '你到了跟我说一声，我下去接你' }]),
];

const members: MemberDTO[] = [
  { user_id: 30011, nickname: '李工', card: '李工', display_name: '李工' },
  { user_id: 30012, nickname: '小陈', card: '小陈', display_name: '小陈' },
  { user_id: 30013, nickname: '王姐', card: '王姐', display_name: '王姐' },
  { user_id: 30014, nickname: '大林', card: null, display_name: '大林' },
  { user_id: SELF_ID, nickname: '我自己', card: null, display_name: '我自己' },
];

let settings: SettingsDTO = {
  ws_url: 'ws://127.0.0.1:3001',
  access_token: '',
  auto_reconnect: true,
  bar_width: 264,
  always_on_top: true,
  locked: false,
  snap_top: true,
  panel_alpha: 0.72,
  bar_alpha: 0.72,
  bubble_alpha: 0.35,
  readability_compensation: true,
  motion: true,
  tab_limit: 5,
  flash_times: 3,
  flash_period_ms: 600,
  flash_on_mention_when_muted: false,
  cache_keep_days: 30,
  cache_limit_bytes: 2 * 1024 * 1024 * 1024,
  cache_clean_on_start: false,
  hotkey_toggle: 'Ctrl+Alt+Q',
  hotkey_mute: 'Ctrl+Alt+M',
  log_level: 'info',
};

let seq = 100000;
let demoIndex = 0;

const DEMO_INCOMING: [number, 0 | 1, string, string][] = [
  [20009, 0, '老王', '这个报错你帮我看下？'],
  [30003, 1, '大林', '周六爬山还剩两个位'],
  [20001, 0, '小陈', '路上堵吗'],
];

/** 让界面"活"起来：装一个定时器，模拟真实消息到达（含提醒决策的假结果） */
function scheduleIncoming(): void {
  window.setInterval(() => {
    const item = DEMO_INCOMING[demoIndex % DEMO_INCOMING.length];
    demoIndex += 1;
    if (!item) return;
    const [peerId, peerType, sender, text] = item;
    const target = conversations.find((c) => c.peer_id === peerId && c.peer_type === peerType);
    if (!target) return;

    const createdAt = Date.now();
    const newMsg = msg(`9${seq++}`, { peer_type: peerType, peer_id: peerId }, 0, peerId, sender, [
      { type: 'text', text },
    ]);
    newMsg.ts = createdAt;
    const key = `${peerType}:${peerId}`;
    messages[key] = [...(messages[key] ?? []), newMsg];

    target.last_msg_time = createdAt;
    target.last_msg_text = text;
    target.last_msg_sender = sender;
    if (!target.is_muted) target.unread_count += 1;

    emit('msg_added', { message: newMsg, conversation: { ...target } });
    emit('conversations', conversations.map((c) => ({ ...c })));
    emit('notify', {
      peer_type: peerType,
      peer_id: peerId,
      flash: !target.is_muted,
      mark: target.is_muted ? 'none' : 'bar',
      claim_bar: !target.is_muted,
    });
  }, 15000);
}

export async function installMockBackend(): Promise<void> {
  window.setTimeout(() => {
    emit('conn_state', { state: 'connected', self_id: SELF_ID });
    emit('conversations', conversations.map((c) => ({ ...c })));
    scheduleIncoming();
  }, 120);
}

/* ------------------------------ 命令实现 ------------------------------ */

export async function mockInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const a = args ?? {};
  const peerType = a['peerType'] as 0 | 1 | undefined;
  const peerId = a['peerId'] as number | undefined;
  const key = peerType !== undefined && peerId !== undefined ? `${peerType}:${peerId}` : null;

  switch (cmd) {
    case 'conn_status':
      return { state: 'connected', self_id: SELF_ID } as T;

    case 'retry_connect':
      emit('conn_state', { state: 'connected', self_id: SELF_ID });
      return undefined as T;

    case 'list_conversations':
      return conversations.map((c) => ({ ...c })) as T;

    case 'load_messages': {
      const beforeSeq = a['beforeSeq'] as number | null;
      const limit = (a['limit'] as number) ?? 30;
      const all = key === null ? [] : (messages[key] ?? []);
      const older = beforeSeq === null ? all : all.filter((m) => (m.seq ?? 0) < beforeSeq);
      return older.slice(-limit) as T;
    }

    case 'send_message': {
      if (key === null) throw new Error('mock: 缺少会话参数');
      const parts = (a['parts'] ?? []) as { kind: string; text?: string; name?: string; path?: string; qq?: number; id?: string }[];
      const text = parts.map((p) => (p.kind === 'text' ? (p.text ?? '') : p.kind === 'image' ? '[图片]' : `@${p.name ?? ''}`)).join('');
      const replyPart = parts.find((p) => p.kind === 'reply');
      const createdAt = Date.now();
      const local: MessageDTO = msg(
        `6${seq++}`,
        { peer_type: peerType!, peer_id: peerId! },
        0,
        SELF_ID,
        '我',
        parts
          .filter((p) => p.kind !== 'reply')
          .map((p) =>
            p.kind === 'image'
              ? ({ type: 'image', path: p.path ?? IMG, sub_type: 0, state: 1 } as const)
              : p.kind === 'at'
                ? ({ type: 'at', qq: p.qq ?? 0, name: p.name ?? '', is_self: false } as const)
                : ({ type: 'text', text: p.text ?? '' } as const),
          ),
        { send_state: 3 },
      );
      local.ts = createdAt;
      local.text = text;
      if (replyPart) {
        local.reply_to = replyPart.id ?? null;
        local.reply_preview = {
          message_id: replyPart.id ?? '',
          sender_name: '李工',
          summary: '（mock）被引用的消息',
          deleted: false,
        };
      }
      messages[key] = [...(messages[key] ?? []), local];

      const target = conversations.find((c) => `${c.peer_type}:${c.peer_id}` === key);
      if (target) {
        target.last_msg_time = createdAt;
        target.last_msg_text = text;
        target.last_msg_sender = '我';
      }
      emit('msg_added', { message: local, conversation: target ? { ...target } : null });

      // 模拟回填真实 message_id（真实实现里由 message_sent.* 事件驱动）
      window.setTimeout(() => {
        const real: MessageDTO = { ...local, message_id: `2${seq++}`, send_state: 0 };
        messages[key] = (messages[key] ?? []).map((m) =>
          m.message_id === local.message_id ? real : m,
        );
        emit('msg_removed', local.message_id);
        emit('msg_added', { message: real, conversation: target ? { ...target } : null });
      }, 700);

      return { message_id: local.message_id } as T;
    }

    case 'retry_send':
      return { message_id: String(a['messageId'] ?? '') } as T;

    case 'mark_read': {
      const target = conversations.find((c) => `${c.peer_type}:${c.peer_id}` === key);
      if (target) {
        target.unread_count = 0;
        target.has_mention = false;
      }
      emit('conversations', conversations.map((c) => ({ ...c })));
      return undefined as T;
    }

    case 'mark_all_read':
      for (const c of conversations) {
        c.unread_count = 0;
        c.has_mention = false;
      }
      emit('conversations', conversations.map((c) => ({ ...c })));
      return undefined as T;

    case 'set_mute': {
      const target = conversations.find((c) => `${c.peer_type}:${c.peer_id}` === key);
      if (target) target.is_muted = Boolean(a['muted']);
      emit('conversations', conversations.map((c) => ({ ...c })));
      return undefined as T;
    }

    case 'delete_message': {
      const id = String(a['messageId'] ?? '');
      for (const k of Object.keys(messages)) {
        messages[k] = (messages[k] ?? []).filter((m) => m.message_id !== id);
      }
      emit('msg_removed', id);
      return undefined as T;
    }

    case 'recall_message':
      return undefined as T;

    case 'add_tab': {
      const target = conversations.find((c) => `${c.peer_type}:${c.peer_id}` === key);
      if (target && target.tab_order === null) {
        const used = conversations
          .filter((c) => c.tab_order !== null)
          .map((c) => c.tab_order as number);
        const limit = settings.tab_limit;
        if (used.length >= limit) {
          const tail = conversations
            .filter((c) => c.tab_order !== null && !c.is_manual_tab)
            .sort((x, y) => (y.tab_order ?? 0) - (x.tab_order ?? 0))[0];
          if (tail) tail.tab_order = null;
        }
        target.tab_order = Math.max(-1, ...used) + 1;
        target.is_manual_tab = true;
      }
      emit('conversations', conversations.map((c) => ({ ...c })));
      return undefined as T;
    }

    case 'remove_tab': {
      const target = conversations.find((c) => `${c.peer_type}:${c.peer_id}` === key);
      if (target) {
        target.tab_order = null;
        target.is_manual_tab = false;
      }
      emit('conversations', conversations.map((c) => ({ ...c })));
      return undefined as T;
    }

    case 'reorder_tabs': {
      const order = (a['order'] ?? []) as string[];
      order.forEach((k, i) => {
        const target = conversations.find((c) => `${c.peer_type}:${c.peer_id}` === k);
        if (target) target.tab_order = i;
      });
      emit('conversations', conversations.map((c) => ({ ...c })));
      return undefined as T;
    }

    case 'refresh_conversations':
      emit('conversations', conversations.map((c) => ({ ...c })));
      return undefined as T;

    case 'list_members': {
      const q = String(a['query'] ?? '');
      return members.filter((m) => q === '' || m.display_name.includes(q)) as T;
    }

    case 'cache_overview': {
      const groups: CacheOverviewDTO['groups'] = [
        {
          peer_type: 1,
          peer_id: 30001,
          name: '大前端交流群',
          bytes: 42 * 1024 * 1024,
          count: 128,
          oldest: NOW - 20 * 86_400_000,
          newest: NOW - 2 * 3600_000,
        },
        {
          peer_type: 0,
          peer_id: 20001,
          name: '小陈',
          bytes: 8 * 1024 * 1024,
          count: 31,
          oldest: NOW - 12 * 86_400_000,
          newest: NOW - 6 * MIN,
        },
      ];
      return {
        total_bytes: groups.reduce((s, g) => s + g.bytes, 0),
        total_count: groups.reduce((s, g) => s + g.count, 0),
        limit_bytes: settings.cache_limit_bytes,
        keep_days: settings.cache_keep_days,
        groups,
      } as T;
    }

    case 'clear_cache':
      return undefined as T;

    case 'get_settings':
      return { ...settings } as T;

    case 'set_setting': {
      const k = String(a['key'] ?? '');
      settings = { ...settings, [k]: a['value'] } as SettingsDTO;
      return undefined as T;
    }

    case 'save_pasted_image':
      return { sha256: `mock-${seq++}`, path: IMG } as T;

    // 真实实现里它决定"该不该闪"，假后端不做提醒判定，所以只需要不抛错
    case 'set_viewing':
      return undefined as T;

    case 'expand_window':
    case 'collapse_window':
      return undefined as T;

    // 真实实现里它会先抑制自动收起、再交给系统拖动；假后端只有窗口内的一层 DOM，
    // 没有可拖动的原生窗口，所以只需要不抛错
    case 'begin_drag':
      return undefined as T;

    case 'window_state':
      return { expanded: false, width: 264, height: 40 } as T;

    case 'exit_app':
      emit('toast', { text: '（mock）这里不会真的退出', kind: 'info' });
      return undefined as T;

    default:
      throw new Error(`mock: 未实现的命令 ${cmd}`);
  }
}
