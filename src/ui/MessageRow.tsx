/**
 * 单条消息行（FR-16 / FR-17 / FR-18 / FR-44 / FR-45）。
 *
 * 头像用「首字 + 由 id 推导的确定色」，**不请求任何网络头像**——
 * 几百个头像的网络与解码开销纯浪费，而且 OneBot 也没有稳定的头像接口（优化清单 #6）。
 */

import { For, Show, createMemo, type JSX } from 'solid-js';
import type { MessageDTO, Seg } from '../state/types';
import * as S from '../state/store';
import * as ipc from '../state/ipc';
import type { MessageRow as Row } from '../core/grouping';
import { IMAGE_BLOCK, IMAGE_CLEANED, IMAGE_FAILED, PLACEHOLDER_TEXT } from '../core/segments';
import { showMenu } from './ContextMenu';

/** 一组低饱和底色，按 sender_id 取，保证同一个人永远是同色 */
const AVATAR_COLORS = [
  '#7F77DD',
  '#1D9E75',
  '#534AB7',
  '#C0703C',
  '#3A7A8C',
  '#8C5A6E',
  '#5F7A3A',
  '#4A6FA5',
];

export function avatarColor(id: number): string {
  const i = Math.abs(id) % AVATAR_COLORS.length;
  return AVATAR_COLORS[i] ?? AVATAR_COLORS[0]!;
}

export function initialOf(name: string | null, id: number): string {
  const n = name?.trim();
  if (n && n.length > 0) return n.slice(0, 1);
  return String(id).slice(-1);
}

interface Props {
  row: Row;
  selfId: number | null;
  /** 点击引用块跳原消息 */
  onJump?: (messageId: string) => void;
}

export function MessageRow(props: Props) {
  const msg = () => props.row.msg;
  const isMe = () => msg().is_self;
  const canQuote = createMemo(() => !msg().is_recalled);

  const copyText = () => {
    const text = msg()
      .segments.map((s) => (s.type === 'text' ? s.text : s.type === 'at' ? `@${s.name ?? s.qq}` : ''))
      .join('');
    void navigator.clipboard?.writeText(text);
  };

  const menuFor = (e: MouseEvent) => {
    const entries: Parameters<typeof showMenu>[1] = [];
    if (msg().text !== null && msg().text !== '') {
      entries.push({ label: '复制文本', action: copyText });
    }
    entries.push({
      label: '引用回复',
      disabled: !canQuote(),
      action: () => S.setQuote(fromMessage(msg())),
    });
    const firstImage = msg().segments.find(
      (s): s is Extract<Seg, { type: 'image' }> => s.type === 'image' && s.path !== null,
    );
    if (firstImage) {
      entries.push({
        label: '查看原图',
        action: () => S.setViewer(ipc.imageUrl(imageRel(msg(), firstImage))),
      });
    }
    entries.push('sep');
    entries.push({
      label: '删除此消息',
      danger: true,
      action: () => void ipc.deleteMessage(msg().message_id),
    });
    showMenu(e, entries);
  };

  return (
    <>
      <Show when={props.row.separator}>
        {(sep) => <div class="day">{sep().text}</div>}
      </Show>

      <div
        class="row"
        classList={{
          me: isMe(),
          cont: props.row.cont,
          recalled: msg().is_recalled,
        }}
      >
        <Show when={props.row.showWho}>
          <div class="av" style={{ background: avatarColor(msg().sender_id) }}>
            {initialOf(msg().sender_name, msg().sender_id)}
          </div>
        </Show>

        <div class="col">
          <Show when={props.row.showWho}>
            <div class="who">{msg().sender_name ?? msg().sender_id}</div>
          </Show>

          <div class="bub" classList={{ atme: msg().is_at_me }} onContextMenu={menuFor}>
            <Show when={msg().reply_to}>
              <div
                class="reply-block"
                onClick={() => {
                  const id = msg().reply_to;
                  if (id !== null) props.onJump?.(id);
                }}
                title="点击跳到原消息"
              >
                <i />
                <div class="reply-body">
                  <Show
                    when={msg().reply_preview}
                    fallback={<span class="reply-deleted">引用的消息已被删除</span>}
                  >
                    {(rp) => (
                      <>
                        <span class="reply-who">
                          {rp().deleted ? '引用' : (rp().sender_name ?? '')}
                        </span>
                        <span class="reply-sum">
                          {rp().deleted ? '引用的消息已被删除' : rp().summary}
                        </span>
                      </>
                    )}
                  </Show>
                </div>
              </div>
            </Show>

            <For each={msg().segments}>
              {(seg) => <Segment seg={seg} selfId={props.selfId} msg={msg()} />}
            </For>
          </div>

          <Show when={msg().is_at_me}>
            <div class="atme-tag">有人 @ 你</div>
          </Show>

          <Show when={msg().is_recalled}>
            <div class="recall-note">
              {recallText(msg())}
            </div>
          </Show>

          <Show when={msg().send_state === 2}>
            <div class="fail-note" onClick={() => void ipc.retrySend(msg().message_id)}>
              发送失败 · 点击重试
            </div>
          </Show>
        </div>
      </div>
    </>
  );
}

function recallText(msg: MessageDTO): string {
  if (msg.recalled_by !== null && msg.recalled_by_name !== null && !msg.is_self) {
    return `${msg.recalled_by_name} 撤回了 ${msg.sender_name ?? ''} 的一条消息`;
  }
  return '已撤回';
}

/** 图片消息的 rel_path 存在 images 里；按顺序对应第 n 个 image 段 */
function imageRel(msg: MessageDTO, seg: Extract<Seg, { type: 'image' }>): string {
  const idx = msg.segments
    .filter((s) => s.type === 'image')
    .findIndex((s) => s === seg);
  const ref = msg.images[idx] ?? msg.images[0];
  return ref?.rel_path ?? seg.path ?? '';
}

function Segment(props: { seg: Seg; selfId: number | null; msg: MessageDTO }) {
  return <>{segmentView(props.seg, props.msg)}</>;
}

/** 把一个规范消息段渲染成 JSX。reply 段不在这里出现——它由引用块单独承载 */
function segmentView(seg: Seg, msg: MessageDTO): JSX.Element {
  switch (seg.type) {
    case 'text':
      return <span class="msg-text">{seg.text}</span>;
    case 'at':
      return <span class="at-token">@{seg.is_self ? '你' : (seg.name ?? seg.qq)}</span>;
    case 'image':
      return <ImageSeg seg={seg} msg={msg} />;
    case 'placeholder':
      return <span class="msg-text">{seg.text || PLACEHOLDER_TEXT[seg.kind]}</span>;
    case 'system':
      return <span class="msg-text">{seg.text}</span>;
    default:
      // reply 段已由引用块渲染，这里不再重复
      return null;
  }
}

function ImageSeg(props: {
  seg: Extract<Seg, { type: 'image' }>;
  msg: MessageDTO;
}) {
  const state = () => props.seg.state;
  const rel = createMemo(() => imageRel(props.msg, props.seg));

  return (
    <Show
      when={state() === 1 && rel() !== ''}
      fallback={
        <span class="msg-text">
          {state() === 4 ? IMAGE_CLEANED : state() === 3 ? IMAGE_FAILED : IMAGE_BLOCK}
        </span>
      }
    >
      <div
        class="thumb"
        onClick={() => S.setViewer(ipc.imageUrl(rel()))}
        title="点击查看原图"
      >
        <img src={ipc.imageUrl(rel())} alt={IMAGE_BLOCK} loading="lazy" draggable={false} />
      </div>
    </Show>
  );
}

/** 把一条消息转成引用摘要（右键「引用回复」用） */
export function fromMessage(msg: MessageDTO): S.Quote {
  return {
    message_id: msg.message_id,
    sender_name: msg.sender_name,
    summary: msg.text ?? '',
    deleted: msg.is_recalled,
  };
}
