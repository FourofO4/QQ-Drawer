/**
 * @ 选人弹层（FR-27）。
 *
 * **不显示头像**，只做文本过滤——几百个头像的网络与解码开销纯浪费（优化清单 #6）。
 * 成员列表来自本地缓存（Rust 侧 24h 过期刷新），打开弹层时才拉。
 */

import { For, Show } from 'solid-js';
import type { MemberDTO } from '../state/types';

interface Props {
  members: readonly MemberDTO[];
  /** 当前高亮项下标 */
  activeIndex: number;
  onPick: (m: MemberDTO) => void;
  onHover: (i: number) => void;
}

export function AtPicker(props: Props) {
  return (
    <div class="at-picker" onMouseDown={(e) => e.preventDefault()}>
      <Show
        when={props.members.length > 0}
        fallback={<div class="at-empty">没有匹配的成员</div>}
      >
        <For each={props.members}>
          {(m, i) => (
            <div
              class="at-item"
              classList={{ active: i() === props.activeIndex }}
              onMouseEnter={() => props.onHover(i())}
              onClick={() => props.onPick(m)}
            >
              <span>{m.display_name}</span>
              <span class="uid">{m.user_id}</span>
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}
