/**
 * 右键菜单（§3.9）。
 *
 * 无边框窗口几乎没有按钮，右键菜单是主要的功能承载入口 —— 所以它必须是全局单例，
 * 任何组件调 `showMenu()` 即可，不需要各自维护弹层。
 */

import { For, Show, createSignal, onCleanup, onMount } from 'solid-js';

export interface MenuItem {
  label: string;
  action: () => void;
  danger?: boolean;
  disabled?: boolean;
}

export type MenuEntry = MenuItem | 'sep';

export interface MenuState {
  x: number;
  y: number;
  entries: MenuEntry[];
}

const [menu, setMenu] = createSignal<MenuState | null>(null);

export function closeMenu(): void {
  setMenu(null);
}

/** 在鼠标位置弹出菜单；越界时自动回收到窗口内 */
export function showMenu(e: MouseEvent, entries: MenuEntry[]): void {
  e.preventDefault();
  e.stopPropagation();
  const itemH = 28;
  const pad = 8;
  const height = entries.reduce((h, it) => h + (it === 'sep' ? 9 : itemH), 0) + pad;
  const width = 168;
  const vw = window.innerWidth;
  const vh = window.innerHeight;

  setMenu({
    x: Math.max(pad, Math.min(e.clientX, vw - width - pad)),
    y: Math.max(pad, Math.min(e.clientY, vh - height - pad)),
    entries,
  });
}

export function ContextMenuHost() {
  const onDown = (e: MouseEvent) => {
    const host = document.getElementById('ctx-host');
    if (host && e.target instanceof Node && host.contains(e.target)) return;
    closeMenu();
  };

  onMount(() => {
    window.addEventListener('mousedown', onDown, true);
    window.addEventListener('blur', closeMenu);
    window.addEventListener('resize', closeMenu);
  });
  onCleanup(() => {
    window.removeEventListener('mousedown', onDown, true);
    window.removeEventListener('blur', closeMenu);
    window.removeEventListener('resize', closeMenu);
  });

  return (
    <Show when={menu()}>
      {(m) => (
        <div
          id="ctx-host"
          class="ctx"
          style={{ left: `${m().x}px`, top: `${m().y}px` }}
          onContextMenu={(e) => e.preventDefault()}
        >
          <For each={m().entries}>
            {(entry) => (
              <Show
                when={entry !== 'sep'}
                fallback={<div class="ctx-sep" />}
              >
                {(() => {
                  const item = entry as MenuItem;
                  return (
                    <div
                      class="ctx-item"
                      classList={{ danger: item.danger === true, disabled: item.disabled === true }}
                      onClick={() => {
                        if (item.disabled === true) return;
                        closeMenu();
                        item.action();
                      }}
                    >
                      {item.label}
                    </div>
                  );
                })()}
              </Show>
            )}
          </For>
        </div>
      )}
    </Show>
  );
}
