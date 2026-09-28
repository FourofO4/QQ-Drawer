/**
 * 展开面板（§3.3）。
 *
 * 结构：标签栏 42px + 消息区（弹性）+ 输入区。溢出层浮在消息区之上、输入区之下。
 * 面板铺满整个窗口——窗口在展开态就是 584×500（§3.1）。
 */

import { Show, onCleanup, onMount } from 'solid-js';
import * as S from '../state/store';
import { requestCollapse } from '../state/events';
import { Tabs } from './Tabs';
import { MessageList } from './MessageList';
import { Overflow } from './Overflow';
import { Composer } from './Composer';
import { Settings } from './Settings';
import { CacheManager } from './CacheManager';
import { ResizeGrip } from './ResizeGrip';

export function Panel() {
  /** 点面板其它区域收回溢出层（FR-12） */
  const onMouseDown = (e: MouseEvent) => {
    if (!S.state.overflowOpen) return;
    const t = e.target;
    if (t instanceof HTMLElement && (t.closest('.overflow') ?? t.closest('.tab.more'))) return;
    S.setOverflowOpen(false);
  };

  /** Esc：先关溢出层，没有溢出层再收起面板（§3.9） */
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== 'Escape') return;
    if (S.state.sheet !== null) {
      S.setSheet(null);
      return;
    }
    if (S.state.overflowOpen) {
      S.setOverflowOpen(false);
      return;
    }
    if (S.state.expanded) {
      e.preventDefault();
      requestCollapse();
    }
  };

  onMount(() => window.addEventListener('keydown', onKey));
  onCleanup(() => window.removeEventListener('keydown', onKey));

  return (
    <div
      class="panel"
      classList={{ in: !S.state.closing, out: S.state.closing }}
      onMouseDown={onMouseDown}
    >
      <Tabs />
      <MessageList />
      <Overflow />
      <Composer />

      {/* 设置 / 缓存管理只在打开时挂载（优化清单 #11） */}
      <Show when={S.state.sheet === 'settings'}>
        <Settings />
      </Show>
      <Show when={S.state.sheet === 'cache'}>
        <CacheManager />
      </Show>

      {/* 尺寸把手（优化 2）：四条边 + 四个角。抓哪条边就动哪条边、对边钉住。
          左 / 上的把手会让窗口原点跟着走 —— Rust 侧落库的是"面板相对锚点的偏移"，
          所以面板仍然跟着折叠条走（拖折叠条换位置时面板自动跟随）。 */}
      <ResizeGrip edge="n" class="grip-n" />
      <ResizeGrip edge="s" class="grip-s" />
      <ResizeGrip edge="w" class="grip-w" />
      <ResizeGrip edge="e" class="grip-e" />
      <ResizeGrip edge="nw" class="grip-nw" />
      <ResizeGrip edge="ne" class="grip-ne" />
      <ResizeGrip edge="sw" class="grip-sw" />
      <ResizeGrip edge="se" class="grip-se" />
    </div>
  );
}
