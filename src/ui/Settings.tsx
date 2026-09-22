/**
 * 设置面板（§4.10 配置项）。
 *
 * 每一项改动都立刻落库（Rust 侧 settings 表），不做"保存"按钮——
 * 这个程序的设置项都很轻，改完即生效比维护脏状态省事。
 */

import { For, Show, createSignal } from 'solid-js';
import * as S from '../state/store';
import * as ipc from '../state/ipc';
import { PANEL_PRESETS } from '../core/theme';
import type { SettingsDTO } from '../state/types';

type Key = keyof SettingsDTO;

export function Settings() {
  const st = () => S.state.settings;
  const [confirmReset, setConfirmReset] = createSignal(false);

  async function save(key: Key, value: unknown): Promise<void> {
    S.patchSettings({ [key]: value });
    await ipc.setSetting(key, value);
  }

  return (
    <div class="sheet">
      <div class="sheet-head">
        <span class="sheet-title">设置</span>
        <button class="btn-icon" onClick={() => S.setSheet(null)} title="关闭">
          ✕
        </button>
      </div>

      <div class="sheet-body">
        <Show when={st()} fallback={<div class="note">正在读取设置…</div>}>
          {(s) => (
            <>
              <div class="grp">
                <h2>连接</h2>
                <div class="field">
                  <label>WebSocket</label>
                  <input
                    type="text"
                    value={s().ws_url}
                    placeholder="ws://127.0.0.1:3001"
                    onChange={(e) => void save('ws_url', e.currentTarget.value.trim())}
                  />
                </div>
                <div class="field">
                  <label>access_token</label>
                  <input
                    type="password"
                    value={s().access_token}
                    placeholder="必填"
                    onChange={(e) => void save('access_token', e.currentTarget.value)}
                  />
                </div>
                <label class="chk">
                  <input
                    type="checkbox"
                    checked={s().auto_reconnect}
                    onChange={(e) => void save('auto_reconnect', e.currentTarget.checked)}
                  />
                  断线自动重连（指数退避）
                </label>
                <div class="note">
                  只监听 <b>127.0.0.1</b>，禁止绑定 0.0.0.0 或做端口映射；连接必须带 token（NFR-11）。
                  改完地址后点折叠条上的“连接已断开 · 点击重试”即可重连。
                </div>
              </div>

              <div class="grp">
                <h2>窗口</h2>
                <div class="field">
                  <label>折叠条宽度</label>
                  <input
                    type="range"
                    min="180"
                    max="420"
                    value={s().bar_width}
                    onInput={(e) => void save('bar_width', Number(e.currentTarget.value))}
                  />
                  <span class="v">{s().bar_width}</span>
                </div>
                <label class="chk">
                  <input
                    type="checkbox"
                    checked={s().always_on_top}
                    onChange={(e) => void save('always_on_top', e.currentTarget.checked)}
                  />
                  始终置顶
                </label>
                <label class="chk">
                  <input
                    type="checkbox"
                    checked={s().snap_top}
                    onChange={(e) => void save('snap_top', e.currentTarget.checked)}
                  />
                  接近屏幕上边缘时吸附
                </label>
                <label class="chk">
                  <input
                    type="checkbox"
                    checked={s().locked}
                    onChange={(e) => {
                      void save('locked', e.currentTarget.checked);
                      S.setLocked(e.currentTarget.checked);
                    }}
                  />
                  锁定展开（点外部不收起）
                </label>
              </div>

              <div class="grp">
                <h2>外观</h2>
                <div class="field">
                  <label>面板底</label>
                  <input
                    type="range"
                    min="30"
                    max="95"
                    value={Math.round(s().panel_alpha * 100)}
                    onInput={(e) => void save('panel_alpha', Number(e.currentTarget.value) / 100)}
                  />
                  <span class="v">{Math.round(s().panel_alpha * 100)}</span>
                </div>
                <div class="btns">
                  <For each={PANEL_PRESETS}>
                    {(p) => (
                      <button
                        class="b"
                        classList={{ on: Math.abs(s().panel_alpha - p.value) < 0.005 }}
                        onClick={() => void save('panel_alpha', p.value)}
                      >
                        {p.label} {Math.round(p.value * 100)}
                      </button>
                    )}
                  </For>
                </div>
                <div class="field" style={{ 'margin-top': '12px' }}>
                  <label>收起条</label>
                  <input
                    type="range"
                    min="15"
                    max="95"
                    value={Math.round(s().bar_alpha * 100)}
                    onInput={(e) => void save('bar_alpha', Number(e.currentTarget.value) / 100)}
                  />
                  <span class="v">{Math.round(s().bar_alpha * 100)}</span>
                </div>
                <div class="btns">
                  <For each={PANEL_PRESETS}>
                    {(p) => (
                      <button
                        class="b"
                        classList={{ on: Math.abs(s().bar_alpha - p.value) < 0.005 }}
                        onClick={() => void save('bar_alpha', p.value)}
                      >
                        {p.label} {Math.round(p.value * 100)}
                      </button>
                    )}
                  </For>
                </div>
                <div class="field" style={{ 'margin-top': '12px' }}>
                  <label>气泡底</label>
                  <input
                    type="range"
                    min="8"
                    max="70"
                    value={Math.round(s().bubble_alpha * 100)}
                    onInput={(e) => void save('bubble_alpha', Number(e.currentTarget.value) / 100)}
                  />
                  <span class="v">{Math.round(s().bubble_alpha * 100)}</span>
                </div>
                <label class="chk">
                  <input
                    type="checkbox"
                    checked={s().readability_compensation}
                    onChange={(e) =>
                      void save('readability_compensation', e.currentTarget.checked)
                    }
                  />
                  可读性补偿
                </label>
                <label class="chk">
                  <input
                    type="checkbox"
                    checked={s().motion}
                    onChange={(e) => void save('motion', e.currentTarget.checked)}
                  />
                  动效
                </label>
                <div class="note">
                  气泡与面板<b>各自独立</b>。开了补偿后，面板越透、底色越深，用来顶住文字对比度；
                  面板色调与气泡色调的计算都在 JS 里完成，<b>不写进 CSS 的 calc()</b>。
                </div>
              </div>

              <div class="grp">
                <h2>会话与提醒</h2>
                <div class="field">
                  <label>标签数量上限</label>
                  <input
                    type="number"
                    min="1"
                    max="12"
                    value={s().tab_limit}
                    onChange={(e) => void save('tab_limit', Number(e.currentTarget.value))}
                  />
                </div>
                <div class="field">
                  <label>闪烁次数</label>
                  <input
                    type="number"
                    min="0"
                    max="6"
                    value={s().flash_times}
                    onChange={(e) => void save('flash_times', Number(e.currentTarget.value))}
                  />
                </div>
                <div class="field">
                  <label>单次时长 ms</label>
                  <input
                    type="number"
                    min="100"
                    max="1000"
                    step="50"
                    value={s().flash_period_ms}
                    onChange={(e) => void save('flash_period_ms', Number(e.currentTarget.value))}
                  />
                </div>
                <label class="chk">
                  <input
                    type="checkbox"
                    checked={s().flash_on_mention_when_muted}
                    onChange={(e) =>
                      void save('flash_on_mention_when_muted', e.currentTarget.checked)
                    }
                  />
                  静音会话被 @ 时仍然闪烁
                </label>
                <div class="note">
                  默认关闭：静音是一个承诺，@ 也不该打破它。标签顺序固定不自动重排，
                  新会话追加到末尾，只有手动拖动才变。
                </div>
              </div>

              <div class="grp">
                <h2>缓存</h2>
                <div class="field">
                  <label>保留天数</label>
                  <input
                    type="number"
                    min="1"
                    max="365"
                    value={s().cache_keep_days}
                    onChange={(e) => void save('cache_keep_days', Number(e.currentTarget.value))}
                  />
                </div>
                <div class="field">
                  <label>容量上限 MB</label>
                  <input
                    type="number"
                    min="100"
                    max="20480"
                    step="100"
                    value={Math.round(s().cache_limit_bytes / 1024 / 1024)}
                    onChange={(e) =>
                      void save('cache_limit_bytes', Number(e.currentTarget.value) * 1024 * 1024)
                    }
                  />
                </div>
                <label class="chk">
                  <input
                    type="checkbox"
                    checked={s().cache_clean_on_start}
                    onChange={(e) => void save('cache_clean_on_start', e.currentTarget.checked)}
                  />
                  启动时清理一次
                </label>
                <div class="btns" style={{ 'margin-top': '10px' }}>
                  <button class="b" onClick={() => S.setSheet('cache')}>
                    打开缓存管理
                  </button>
                </div>
              </div>

              <div class="grp">
                <h2>高级</h2>
                <div class="field">
                  <label>展开快捷键</label>
                  <input
                    type="text"
                    value={s().hotkey_toggle}
                    onChange={(e) => void save('hotkey_toggle', e.currentTarget.value.trim())}
                  />
                </div>
                <div class="field">
                  <label>静音快捷键</label>
                  <input
                    type="text"
                    value={s().hotkey_mute}
                    onChange={(e) => void save('hotkey_mute', e.currentTarget.value.trim())}
                  />
                </div>
                <div class="field">
                  <label>日志级别</label>
                  <input
                    type="text"
                    value={s().log_level}
                    onChange={(e) => void save('log_level', e.currentTarget.value.trim())}
                  />
                </div>
                <div class="note">
                  全局快捷键是硬需求：窗口 <code>skipTaskbar</code> 后同时从 <code>Alt+Tab</code> 消失，
                  不设快捷键就只剩托盘入口。
                </div>
              </div>

              <div class="grp">
                <h2>数据</h2>
                <div class="btns">
                  <button
                    class="b danger"
                    onClick={() => setConfirmReset(true)}
                  >
                    清空全部本地数据
                  </button>
                </div>
                <Show when={confirmReset()}>
                  <div class="warn">
                    这会删除本机数据库、全部图片缓存与设置，<b>不可恢复</b>。
                    本库是消息的唯一长期副本（NapCat 侧只留约 5000 条且受 LRU 管理）。
                    <div class="btns" style={{ 'margin-top': '8px' }}>
                      <button
                        class="b danger"
                        onClick={() => {
                          void ipc.clearCache('all');
                          setConfirmReset(false);
                        }}
                      >
                        确认清空
                      </button>
                      <button class="b" onClick={() => setConfirmReset(false)}>
                        取消
                      </button>
                    </div>
                  </div>
                </Show>
              </div>
            </>
          )}
        </Show>
      </div>
    </div>
  );
}
