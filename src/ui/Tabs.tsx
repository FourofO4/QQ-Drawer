/**
 * 标签栏（§3.3 / FR-11 / FR-12 / FR-13）。
 *
 * 两条容易走偏的规则：
 *  1. 标签顺序**固定不自动重排** —— 新会话追加到末尾，只有用户手动拖动才变（FR-11）；
 *  2. `···` 是溢出入口，向下展开覆盖消息区，**不改变窗口尺寸、不遮输入框**（FR-12）。
 * 顺带承担顶栏空白区的拖动职责（面板顶部空白区可拖窗）。
 */

import { For, Show, createSignal, createMemo, createEffect } from 'solid-js';
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

  /**
   * 标签栏横滚：把竖直滚轮映射成横向。
   *
   * 标签多到装不下时 `.tabs-drag` 是横滚的（见 global.css），而 40px 高的条里
   * 放一条滚动条会把 26px 的标签挤变形，所以滚动条被隐藏了 —— 滚轮就是唯一的滚动入口，
   * 没这一段的话，超出的标签只能靠触摸板横滑或键盘够到。
   */
  const onWheel = (e: WheelEvent & { currentTarget: HTMLElement }) => {
    const el = e.currentTarget;
    if (el.scrollWidth <= el.clientWidth) return;
    // 触摸板的横向滑动给的是 deltaX，鼠标滚轮给的是 deltaY，谁大用谁
    const d = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
    if (d === 0) return;
    e.preventDefault();
    el.scrollLeft += d;
  };

  /** 切到某个会话时把它滚进视野 —— 横滚之后别让用户找不到自己在哪个标签上 */
  createEffect(() => {
    const key = activeKey();
    if (key === null) return;
    const el = document.querySelector<HTMLElement>(`[data-tab-key="${CSS.escape(key)}"]`);
    el?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  });

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
    // 落在标签 / 按钮 / 尺寸把手上的按下由它们自己（或各自的 onMouseDown）处理，
    // 这里只接管真正的空白区。没有这道闸，拖标签会**同时**触发窗口拖动 ——
    // 两个 mousemove 处理器一起跑，标签拖到一半窗口整块跟着位移。
    // `.grip` 同理：顶边那条 5px 的尺寸把手就压在标签栏上，漏掉它的话
    // 拖顶边会变成"改高度 + 挪窗口"两件事一起做。
    if ((e.target as HTMLElement | null)?.closest('.tab, button, .composer, .grip')) return;
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
      <div class="tabs-drag" onWheel={onWheel}>
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
