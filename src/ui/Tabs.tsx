/**
 * 标签栏（§3.3 / FR-11 / FR-12 / FR-13）。
 *
 * 两条容易走偏的规则：
 *  1. 标签顺序**固定不自动重排** —— 新会话追加到末尾，只有用户手动拖动才变（FR-11）；
 *  2. `···` 是溢出入口，向下展开覆盖消息区，**不改变窗口尺寸、不遮输入框**（FR-12）。
 * 顺带承担顶栏空白区的拖动职责（面板顶部空白区可拖窗）。
 */

import { For, Show, createSignal, createMemo } from 'solid-js';
import * as S from '../state/store';
import * as ipc from '../state/ipc';
import { peerKey } from '../core/preview';
import { showMenu } from './ContextMenu';
import { ensureMessages } from '../state/events';
import type { ConversationDTO } from '../state/types';

const DRAG_THRESHOLD = 6;

export function Tabs() {
  const [dragKey, setDragKey] = createSignal<string | null>(null);
  const [dropIndex, setDropIndex] = createSignal<number | null>(null);

  const list = S.tabs;
  const activeKey = () => S.state.current;

  /** `···` 整体留轻微痕迹：内部只要有未读就亮（FR-15，不做数字） */
  const moreUnread = createMemo(() =>
    S.overflowList().some((c) => c.unread_count > 0 && !c.is_muted),
  );

  const pick = (conv: ConversationDTO) => {
    S.selectConversation(conv);
    void ensureMessages(conv);
  };

  const menuFor = (e: MouseEvent, conv: ConversationDTO) => {
    showMenu(e, [
      {
        label: conv.is_muted ? '取消本会话静音' : '本会话静音',
        action: () => void ipc.setMute(conv.peer_type, conv.peer_id, !conv.is_muted),
      },
      {
        label: '移出标签',
        action: () => void ipc.removeTab(conv.peer_type, conv.peer_id),
      },
      {
        label: '标记已读',
        action: () => void ipc.markRead(conv.peer_type, conv.peer_id),
      },
      'sep',
      {
        label: '清空本会话本地记录',
        danger: true,
        action: () => void ipc.clearCache('peer', {
          peerType: conv.peer_type,
          peerId: conv.peer_id,
          days: 0,
        }),
      },
    ]);
  };

  /**
   * 拖拽排序：按住横向拖动。
   * 实现上只在 mouseup 时提交一次新顺序，拖动过程只做视觉反馈——避免频繁写库。
   */
  const startSort = (e: MouseEvent, conv: ConversationDTO) => {
    if (e.button !== 0) return;
    // 别让这次按下冒泡到 `.tabs` 的 dragWindow —— 否则"拖标签排序"会变成
    // "拖标签排序 + 拖动整个窗口"，两个 mousemove 处理器抢同一串鼠标事件。
    e.stopPropagation();
    const key = peerKey(conv);
    const originX = e.clientX;
    let armed = false;

    const onMove = (ev: MouseEvent) => {
      if (!armed) {
        if (Math.abs(ev.clientX - originX) < DRAG_THRESHOLD) return;
        armed = true;
        setDragKey(key);
      }
      const nodes = Array.from(
        document.querySelectorAll<HTMLElement>('[data-tab-key]'),
      );
      let idx = nodes.length;
      for (let i = 0; i < nodes.length; i += 1) {
        const rect = nodes[i]!.getBoundingClientRect();
        if (ev.clientX < rect.left + rect.width / 2) {
          idx = i;
          break;
        }
      }
      setDropIndex(idx);
    };

    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
      const target = dropIndex();
      const dragging = dragKey();
      setDragKey(null);
      setDropIndex(null);
      if (!armed || dragging === null || target === null) return;

      const order = list().map((c) => peerKey(c));
      const from = order.indexOf(dragging);
      if (from < 0) return;
      order.splice(from, 1);
      // target 是「插入到第几个之前」，移除 from 后下标要左移
      const to = target > from ? target - 1 : target;
      order.splice(Math.max(0, Math.min(to, order.length)), 0, dragging);
      void ipc.reorderTabs(order);
    };

    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  /** 顶栏空白区拖动窗口 */
  const dragWindow = (e: MouseEvent) => {
    if (e.button !== 0) return;
    // 落在标签 / 按钮上的按下由它们自己（或各自的 onMouseDown）处理，
    // 这里只接管真正的空白区。没有这道闸，拖标签会**同时**触发窗口拖动 ——
    // 两个 mousemove 处理器一起跑，标签拖到一半窗口整块跟着位移。
    if ((e.target as HTMLElement | null)?.closest('.tab, button, .composer')) return;
    const x0 = e.clientX;
    const y0 = e.clientY;
    const onMove = (ev: MouseEvent) => {
      if (Math.abs(ev.clientX - x0) + Math.abs(ev.clientY - y0) <= 3) return;
      window.removeEventListener('mousemove', onMove);
      void ipc.beginDrag();
    };
    const onUp = () => window.removeEventListener('mousemove', onMove);
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  const toggleLock = (e: MouseEvent) => {
    e.stopPropagation();
    const next = !S.state.locked;
    S.setLocked(next);
    void ipc.setSetting('locked', next);
  };

  return (
    <div class="tabs" onMouseDown={dragWindow}>
      <div class="tabs-drag">
        <For each={list()}>
          {(conv, i) => (
            <div
              class="tab"
              data-tab-key={peerKey(conv)}
              classList={{
                active: peerKey(conv) === activeKey(),
                muted: conv.is_muted,
                unread:
                  conv.unread_count > 0 &&
                  !conv.is_muted &&
                  peerKey(conv) !== activeKey(),
                dragging: dragKey() === peerKey(conv),
                'drop-target': dragKey() !== null && dropIndex() === i(),
              }}
              onMouseDown={(e) => startSort(e, conv)}
              onClick={(e) => {
                e.stopPropagation();
                pick(conv);
              }}
              onContextMenu={(e) => menuFor(e, conv)}
              title={conv.name}
            >
              <span class="dot" />
              <span class="tab-label">{conv.name}</span>
              <Show when={conv.is_muted}>
                <em>静音</em>
              </Show>
            </div>
          )}
        </For>

        <Show when={S.overflowList().length > 0}>
          <div
            class="tab more"
            classList={{ unread: moreUnread() }}
            onClick={(e) => {
              e.stopPropagation();
              S.toggleOverflow();
            }}
          >
            ···
          </div>
        </Show>
      </div>

      <button
        class="btn-icon btn-lock"
        classList={{ on: S.state.locked }}
        onClick={toggleLock}
        title={S.state.locked ? '已锁定：点击抽屉外部不会自动收起' : '未锁定：点击抽屉外部自动收起'}
      >
        锁定
      </button>
      <button
        class="btn-icon"
        onClick={(e) => {
          e.stopPropagation();
          void ipc.collapse();
          S.setExpanded(false);
        }}
        title="收起"
      >
        ▲
      </button>
    </div>
  );
}
