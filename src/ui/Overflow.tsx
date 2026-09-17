/**
 * 溢出层 `···`（§3.3 / FR-12 / FR-13）。
 *
 * 向下展开覆盖消息区，**不改变窗口尺寸、不遮输入框**（输入区 z-index:2 顶在上面）。
 * 背景用近乎不透明的深色，保证盖在消息之上仍然清晰。
 */

import { For, Show } from 'solid-js';
import * as S from '../state/store';
import * as ipc from '../state/ipc';
import { summarize } from '../core/segments';
import { showMenu } from './ContextMenu';
import { ensureMessages } from '../state/events';
import type { ConversationDTO } from '../state/types';

export function Overflow() {
  const items = S.overflowList;

  const lastText = (c: ConversationDTO) =>
    summarize([{ type: 'text', text: c.last_msg_text ?? '' }]);

  const choose = (conv: ConversationDTO) => {
    S.selectConversation(conv);
    void ensureMessages(conv);
    S.setOverflowOpen(false);
  };

  const pin = (e: MouseEvent, conv: ConversationDTO) => {
    e.stopPropagation();
    // 手动加入标签：优先级最高，由 Rust 挤掉队尾（FR-13）
    void ipc.addTab(conv.peer_type, conv.peer_id);
    S.setOverflowOpen(false);
  };

  const menuFor = (e: MouseEvent, conv: ConversationDTO) => {
    const inTabs = conv.tab_order !== null;
    showMenu(e, [
      {
        label: '加入标签',
        disabled: inTabs,
        action: () => void ipc.addTab(conv.peer_type, conv.peer_id),
      },
      {
        label: conv.is_muted ? '取消本会话静音' : '本会话静音',
        action: () => void ipc.setMute(conv.peer_type, conv.peer_id, !conv.is_muted),
      },
      {
        label: '移出标签',
        disabled: !inTabs,
        action: () => void ipc.removeTab(conv.peer_type, conv.peer_id),
      },
    ]);
  };

  return (
    <Show when={S.state.overflowOpen}>
      <div class="overflow open" onMouseDown={(e) => e.stopPropagation()}>
        <div class="ov-head">未放进标签栏的会话</div>
        <For each={items()}>
          {(conv) => (
            <div
              class="ov-item"
              classList={{
                unread: conv.unread_count > 0 && !conv.is_muted,
                muted: conv.is_muted,
              }}
              onClick={() => choose(conv)}
              onContextMenu={(e) => menuFor(e, conv)}
            >
              <span class="ov-mark" />
              <span class="ov-name">{conv.name}</span>
              <span class="ov-last">{lastText(conv)}</span>
              <span class="ov-pin" onClick={(e) => pin(e, conv)}>
                加入标签
              </span>
            </div>
          )}
        </For>
        <Show when={items().length === 0}>
          <div class="ov-head">没有其它会话</div>
        </Show>
      </div>
    </Show>
  );
}
