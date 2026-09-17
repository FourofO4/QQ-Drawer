/**
 * 折叠条（§3.2 / FR-02 / FR-33）。
 *
 * 它铺满整个窗口（窗口在折叠态就是 264×40），所以这里的一切都是"窗口内容"而不是"窗口里的一个控件"。
 *
 * 三条刻意为之、别改回去的规则：
 *  1. 只有「谁：说了什么」，**无头像、无图标、无未读数字**；
 *  2. **不做自适应宽度** —— 内容多长都不改窗口尺寸，否则新消息会触发 DWM 背景模糊重算（§3.2）；
 *  3. 闪烁**只改折叠条自身的 CSS 变量**，绝不碰窗口 alpha / 分层属性（§3.5）。
 */

import { Show, createMemo } from 'solid-js';
import * as S from '../state/store';
import * as ipc from '../state/ipc';
import { BAR_TEXT, barHasMark, composeBar, pickBarConversation } from '../core/preview';
import { showMenu } from './ContextMenu';
import { openPanel } from '../state/events';

/** 位移 > 3px 才算拖动，拖动后要抑制随后的 click（踩坑 #7） */
const DRAG_THRESHOLD = 3;

export function Bar() {
  const conv = createMemo(() => pickBarConversation(S.state.conversations));
  const content = createMemo(() => {
    const c = conv();
    return c === null ? null : composeBar(c);
  });

  const connected = () => S.state.conn === 'connected';
  const flashing = () => {
    const c = conv();
    return c !== null && S.state.flashKey === `${c.peer_type}:${c.peer_id}`;
  };
  const unread = () => {
    const c = conv();
    if (c === null) return false;
    // 展开且正在看这个会话时不留痕（FR-31 / FR-32）
    const viewing = S.state.expanded ? `${c.peer_type}:${c.peer_id}` === S.state.current : false;
    return barHasMark(c, viewing ? S.state.current : null);
  };

  let dragged = false;
  let start: { x: number; y: number } | null = null;

  const onMouseDown = (e: MouseEvent) => {
    if (e.button !== 0) return;
    dragged = false;
    start = { x: e.clientX, y: e.clientY };
    // 交给系统拖动——但只在超过阈值后调用，否则"一拖就展开"
    const onMove = (ev: MouseEvent) => {
      if (start === null) return;
      const dx = Math.abs(ev.clientX - start.x);
      const dy = Math.abs(ev.clientY - start.y);
      if (dx + dy > DRAG_THRESHOLD) {
        dragged = true;
        start = null;
        cleanup();
        void ipc.startDragging();
      }
    };
    const cleanup = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    const onUp = () => {
      start = null;
      cleanup();
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  const onClick = () => {
    if (dragged) {
      dragged = false;
      return;
    }
    if (!connected()) {
      void ipc.retryConnect();
      return;
    }
    void openPanel();
  };

  const onContextMenu = (e: MouseEvent) => {
    const c = conv();
    const entries: Parameters<typeof showMenu>[1] = [];
    if (c !== null) {
      entries.push({
        label: c.is_muted ? '取消本会话静音' : '本会话静音',
        action: () => void ipc.setMute(c.peer_type, c.peer_id, !c.is_muted),
      });
      entries.push({
        label: '标记已读',
        action: () => void ipc.markRead(c.peer_type, c.peer_id),
      });
      entries.push('sep');
    }
    entries.push({
      label: S.state.locked ? '取消锁定展开' : '锁定展开',
      action: () => {
        const next = !S.state.locked;
        S.setLocked(next);
        void ipc.setSetting('locked', next);
      },
    });
    entries.push({ label: '设置', action: () => S.setSheet('settings') });
    entries.push({ label: '清空缓存', action: () => S.setSheet('cache') });
    entries.push('sep');
    entries.push({ label: '退出', danger: true, action: () => void ipc.exitApp() });
    showMenu(e, entries);
  };

  return (
    <div
      class="bar"
      classList={{
        unread: unread(),
        flashing: flashing(),
        error: !connected(),
        disconnected: !connected(),
      }}
      onMouseDown={onMouseDown}
      onClick={onClick}
      onContextMenu={onContextMenu}
      onAnimationEnd={() => S.stopFlash()}
      title={connected() ? undefined : '点击重试连接'}
    >
      <div class="bar-mark" />
      <div class="bar-text">
        <Show when={content()} fallback={<span class="bar-name">{barFallback()}</span>}>
          {(c) => (
            <>
              <span class="bar-name">{c().name}</span>
              <span class="bar-body">{c().body}</span>
            </>
          )}
        </Show>
      </div>
    </div>
  );
}

function barFallback(): string {
  if (S.state.conn === 'connecting') return BAR_TEXT.connecting;
  if (S.state.conn === 'disconnected') return BAR_TEXT.disconnected;
  return BAR_TEXT.empty;
}
