/**
 * 缓存管理（FR-42 / FR-43）。
 *
 * 这一页的存在理由很硬：**本库是消息的唯一长期副本**。
 * NapCat 侧只留约 5000 条、文件标识同样受 LRU 管理，清理 = 永久丢失。
 * 所以清理动作必须二次确认，并且明说"这是图片的唯一副本，清除后无法恢复"。
 */

import { For, Show, createSignal, onMount } from 'solid-js';
import * as S from '../state/store';
import * as ipc from '../state/ipc';
import type { CacheOverviewDTO } from '../state/types';

type Scope = 'peer' | 'age' | 'all';

export function CacheManager() {
  const [data, setData] = createSignal<CacheOverviewDTO | null>(null);
  const [pending, setPending] = createSignal<Scope | null>(null);

  const load = async () => setData(await ipc.cacheOverview());

  onMount(() => void load());

  const mb = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(1)} MB`;

  const run = async (scope: Scope) => {
    const conv = S.currentConversation();
    await ipc.clearCache(
      scope,
      scope === 'peer' && conv !== null
        ? { peerType: conv.peer_type, peerId: conv.peer_id, days: 0 }
        : undefined,
    );
    setPending(null);
    await load();
    S.showToast('缓存已清理');
  };

  const targetName = () => S.currentConversation()?.name ?? '当前会话';

  return (
    <div class="sheet">
      <div class="sheet-head">
        <span class="sheet-title">缓存管理</span>
        <button class="btn-icon" onClick={() => S.setSheet(null)} title="关闭">
          ✕
        </button>
      </div>

      <div class="sheet-body">
        <Show when={data()} fallback={<div class="note">正在统计…</div>}>
          {(d) => (
            <>
              <div class="grp">
                <h2>总览</h2>
                <div class="field">
                  <label>图片占用</label>
                  <span class="v" style={{ width: 'auto', color: 'var(--t2)' }}>
                    {mb(d().total_bytes)} / {mb(d().limit_bytes)}
                  </span>
                </div>
                <div class="field">
                  <label>图片数量</label>
                  <span class="v" style={{ width: 'auto', color: 'var(--t2)' }}>
                    {d().total_count} 张
                  </span>
                </div>
                <div class="field">
                  <label>保留天数</label>
                  <span class="v" style={{ width: 'auto', color: 'var(--t2)' }}>
                    {d().keep_days} 天
                  </span>
                </div>
                <div class="note">
                  消息正文<b>永久保留</b>，不做自动删除——那会让历史出现空洞。占空间的主要是图片，
                  所以只对图片做 LRU。
                </div>
              </div>

              <div class="grp">
                <h2>按会话</h2>
                <For each={d().groups}>
                  {(g) => (
                    <div class="cache-row">
                      <span class="n">{g.name}</span>
                      <span class="s">{mb(g.bytes)}</span>
                      <span class="s">{g.count} 张</span>
                    </div>
                  )}
                </For>
                <Show when={d().groups.length === 0}>
                  <div class="note">还没有落盘的图片。</div>
                </Show>
              </div>

              <div class="grp">
                <h2>清理</h2>
                <div class="btns">
                  <button class="b" onClick={() => setPending('peer')}>
                    按会话 · {targetName()}
                  </button>
                  <button class="b" onClick={() => setPending('age')}>
                    按时间 · {d().keep_days} 天前
                  </button>
                  <button class="b danger" onClick={() => setPending('all')}>
                    全量清理
                  </button>
                </div>
              </div>

              <Show when={pending()}>
                {(p) => (
                  <div class="warn">
                    将被清除的是图片文件：<b>这是图片的唯一副本，清除后无法恢复。</b>
                    NapCat 侧同样受 LRU 管理，事后无从回查。被清理的图片消息会显示
                    <code>[图片已清除]</code>，文本消息不受影响。
                    <div class="btns" style={{ 'margin-top': '8px' }}>
                      <button class="b danger" onClick={() => void run(p())}>
                        确认清理
                      </button>
                      <button class="b" onClick={() => setPending(null)}>
                        取消
                      </button>
                    </div>
                  </div>
                )}
              </Show>
            </>
          )}
        </Show>
      </div>
    </div>
  );
}
