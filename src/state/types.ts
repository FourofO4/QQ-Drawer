/**
 * 前后端共用模型的 TypeScript 侧定义（DTO）。
 *
 * 这些类型与 `src-tauri/src/model.rs`、`src-tauri/src/store/*.rs` 一一对应，
 * 是 IPC 的唯一契约。前端不持有业务状态，只渲染 Rust 推来的快照与增量（§4.1）。
 */

/** 0 = 私聊，1 = 群聊 */
export type PeerType = 0 | 1;

export interface PeerRef {
  peer_type: PeerType;
  peer_id: number;
}

/** 图片落盘状态（对应 message.image_state） */
export const enum ImageState {
  None = 0,
  Ready = 1,
  Downloading = 2,
  Failed = 3,
  Cleaned = 4,
}
export type ImageStateValue = 0 | 1 | 2 | 3 | 4;

/** 发送状态（对应 message.send_state） */
export const enum SendState {
  Confirmed = 0,
  Sending = 1,
  Failed = 2,
  Local = 3,
}
export type SendStateValue = 0 | 1 | 2 | 3;

export type ConnectionState = 'connecting' | 'connected' | 'disconnected';

/**
 * 规范消息段。
 *
 * OneBot 的原始字段一律不出现在这里——协议的一切形态都终结在 Rust 的 `ob` 层（NFR-13）。
 * 解析不了的一律降级为 `placeholder`，并且原始 JSON 已经入库，日后可补渲染。
 */
export type Seg =
  | { type: 'text'; text: string }
  | {
      type: 'image';
      /** 本地绝对路径；未落盘时为 null */
      path: string | null;
      sub_type: number;
      state: ImageStateValue;
    }
  | { type: 'at'; qq: number; name: string | null; is_self: boolean }
  | { type: 'reply'; id: string }
  | {
      type: 'placeholder';
      kind:
        | 'face'
        | 'mface'
        | 'record'
        | 'video'
        | 'file'
        | 'card'
        | 'forward'
        | 'unknown';
      /** 已本地化的展示文案，如 `[表情]`（§3.10） */
      text: string;
    }
  | { type: 'system'; text: string };

export interface ImageRef {
  sha256: string;
  /** 相对 media 目录的路径，前端拼成 convertFileSrc 后可直接喂给 <img> */
  rel_path: string;
  width: number | null;
  height: number | null;
  sub_type: number;
}

/** 引用块内容；被引用消息已被删除时为 null（FR-45） */
export interface ReplyPreview {
  message_id: string;
  sender_name: string | null;
  /** 单行摘要 */
  summary: string;
  deleted: boolean;
}

export interface MessageDTO {
  message_id: string;
  peer_type: PeerType;
  peer_id: number;
  /** 会话内自增序号，本地生成，用于稳定排序 */
  seq: number | null;
  ts: number;
  sender_id: number;
  sender_name: string | null;
  is_self: boolean;
  /** 纯文本合并结果（搜索与折叠条预览用） */
  text: string | null;
  segments: Seg[];
  reply_to: string | null;
  reply_preview: ReplyPreview | null;
  is_at_me: boolean;
  has_image: boolean;
  image_state: ImageStateValue;
  images: ImageRef[];
  is_recalled: boolean;
  recalled_by: number | null;
  recalled_by_name: string | null;
  send_state: SendStateValue;
}

export interface ConversationDTO {
  peer_type: PeerType;
  peer_id: number;
  /** 展示名：备注 > 群名片 > 昵称/群名（FR-14） */
  name: string;
  last_msg_time: number | null;
  last_msg_text: string | null;
  last_msg_sender: string | null;
  /** 未读数；静音会话同样计数（FR-30） */
  unread_count: number;
  has_mention: boolean;
  /** NULL = 不在标签栏 */
  tab_order: number | null;
  /** 手动加入标签，永不被自动挤掉（FR-13） */
  is_manual_tab: boolean;
  is_muted: boolean;
  /** 群临时会话，会话名要标注「群临时」（§4.5） */
  is_temp?: boolean;
}

export interface MemberDTO {
  user_id: number;
  nickname: string | null;
  card: string | null;
  /** 备注优先，其次群名片，再次昵称 */
  display_name: string;
}

export interface CacheStatDTO {
  peer_type: PeerType;
  peer_id: number;
  name: string;
  bytes: number;
  count: number;
  oldest: number | null;
  newest: number | null;
}

export interface CacheOverviewDTO {
  total_bytes: number;
  total_count: number;
  limit_bytes: number;
  keep_days: number;
  groups: CacheStatDTO[];
}

export interface SettingsDTO {
  ws_url: string;
  access_token: string;
  auto_reconnect: boolean;
  bar_width: number;
  always_on_top: boolean;
  locked: boolean;
  snap_top: boolean;
  panel_alpha: number;
  bar_alpha: number;
  bubble_alpha: number;
  readability_compensation: boolean;
  motion: boolean;
  tab_limit: number;
  flash_times: number;
  flash_period_ms: number;
  flash_on_mention_when_muted: boolean;
  cache_keep_days: number;
  cache_limit_bytes: number;
  cache_clean_on_start: boolean;
  hotkey_toggle: string;
  hotkey_mute: string;
  log_level: string;
}

/**
 * 提醒决策结果 —— 纯函数 `sched::notify` 的输出（§3.5）。
 * 前端只负责把这几个字段翻译成 CSS 类，不做任何判定。
 */
export interface NotifyIntent {
  peer_type: PeerType;
  peer_id: number;
  /** 折叠条是否要闪烁 */
  flash: boolean;
  /** 在哪里留痕 */
  mark: 'none' | 'bar' | 'tab' | 'overflow';
  /** 折叠条是否被这条消息抢占 */
  claim_bar: boolean;
}
