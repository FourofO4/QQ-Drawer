/**
 * 应用根组件。
 *
 * 窗口形态与内容一一对应：折叠态渲染 `Bar`，展开态渲染 `Panel`（§3.1）。
 * 窗口尺寸/位置由 Rust 掌握，这里**永远不直接改窗口几何**（§4.1）。
 */

import { Show, createEffect, onMount } from 'solid-js';
import * as S from './state/store';
import * as ipc from './state/ipc';
import { initEvents, resyncWindowState } from './state/events';
import { applyThemeVars, themeVars } from './core/theme';
import { Bar } from './ui/Bar';
import { Panel } from './ui/Panel';
import { ContextMenuHost } from './ui/ContextMenu';

export function App() {
  onMount(() => {
    void bootstrap();
    void initEvents();
  });
  /** 外观改动立刻反映到 CSS 变量（颜色在 JS 里算好，见 core/theme.ts） */
  createEffect(() => {
    const s = S.state.settings;
    if (s === null) return;
    applyThemeVars(
      themeVars({
        panelAlpha: s.panel_alpha,
        barAlpha: s.bar_alpha,
        bubbleAlpha: s.bubble_alpha,
        compensation: s.readability_compensation,
      }),
      document.documentElement,
    );
    document.documentElement.dataset.motion = s.motion ? 'on' : 'off';
  });

  /**
   * 把「我正在看哪个会话」同步给 Rust。
   *
   * 提醒决策完全依赖它（FR-31：正在看的会话来消息一律不闪），
   * 重连补齐也靠它决定补哪一个会话。收起时传 null —— 折叠态下没有"正在看"这个概念。
   */
  createEffect(() => {
    const expanded = S.state.expanded;
    const conv = expanded ? S.currentConversation() : null;
    if (conv === null) void ipc.setViewing(null, null);
    else void ipc.setViewing(conv.peer_type, conv.peer_id);
  });

  return (
    <>
      <Show when={S.state.expanded} fallback={<Bar />}>
        <Panel />
      </Show>

      <ContextMenuHost />

      {/* 图片放大：点击缩略图看原图 */}
      <Show when={S.state.viewer}>
        {(url) => (
          <div class="viewer" onClick={() => S.setViewer(null)}>
            <img src={url()} alt="图片" />
          </div>
        )}
      </Show>
    </>
  );
}

async function bootstrap(): Promise<void> {
  const settings = await ipc.getSettings();
  S.setSettings(settings);
  S.setLocked(settings.locked);
  document.documentElement.dataset.motion = settings.motion ? 'on' : 'off';

  const info = await ipc.connStatus();
  S.setConn(info.state);
  S.setSelfId(info.self_id);

  const conversations = await ipc.listConversations();
  S.setConversations(conversations);

  // 窗口形态以 Rust 为准对齐一次：万一上次退出时留下了错配，
  // 这次启动就把它纠正过来，而不是等用户"再点一下"。
  await resyncWindowState();

  // 默认选中：标签栏第一个；没有标签就选折叠条那一条
  if (S.state.current === null && conversations.length > 0) {
    const first = S.tabs()[0] ?? conversations[0];
    if (first) S.selectConversation(first);
  }
}
