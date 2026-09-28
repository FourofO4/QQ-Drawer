/**
 * 面板尺寸把手（优化 2）。
 *
 * 八个把手共用这一个组件：四条边 + 四个角。抓哪条边就动哪条边，**对边钉住**
 * （拖右边缘时左边缘不动，拖左上角时右下角不动）—— 这是原生窗口的行为，
 * 也是用户的固有预期。
 *
 * 四条边都要有：只有右 / 下两条的话，面板只能往右下长，被推到屏幕边上就没法调了。
 * 左、上的把手会移动窗口原点，所以 Rust 侧落库的是"面板相对锚点的偏移"
 * （`panel_dx` / `panel_dy`），面板仍然跟着折叠条走。
 *
 * 四条实现要点：
 *  1. **位移必须用屏幕坐标（`screenX` / `screenY`）**，绝不能用 `clientX` / `clientY`。
 *     后者是"相对窗口左下角"的坐标，而拖拽本身就在移动窗口 —— 窗口一动，同一个屏幕点
 *     的相对坐标就变了，于是"位移变大 → 窗口移动更多 → 位移更大"自我放大。
 *     上一版就是这么坏的：拖一下右边把手，面板一路缩成 360×500 的竖屏比例，
 *     松手还把这个错值写进库。屏幕坐标与窗口位置无关，天然没有这条回路。
 *  2. **一帧最多一次 IPC**（rAF 合并）：指针事件的频率远高于帧率，不合并就是在给
 *     IPC 灌垃圾；而 `preview_panel_rect` 本来就是"一帧动一次窗口"，也只能跟上帧率。
 *  3. **指针捕获**：往内拖时窗口会缩小到指针之外，没有捕获就收不到 pointermove，
 *     拖拽会中途"冻住"。
 *  4. **没动过就不写库**：`moved` 闸住"点一下把手"这种情况，否则一次误触就会
 *     把尺寸落库（而且此时窗口其实没变，等于白写一条脏数据）。
 *
 * ⚠️ 这里逐帧改窗口尺寸，是对 §3.4 关键规则 3 的**有意例外**，理由写在
 * `window::preview_panel_rect` 的注释里（那条规则针对的是应用自己驱动的时间轴动画）。
 * 要退化成"松手才落地"，改这里就够：`flush` 里别发 `previewPanelRect`，
 * 只在 `pointerup` 里发 `endResize`。**别在 Rust 侧加节流**，这里已经按帧合并过了。
 */

import { onCleanup } from 'solid-js';
import * as ipc from '../state/ipc';
import {
  rectFromDrag,
  toPhysical,
  type Delta,
  type DragBounds,
  type Edge,
  type Rect,
} from '../core/resize';

interface Props {
  edge: Edge;
  class: string;
}

const same = (a: Rect, b: Rect) =>
  a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h;

export function ResizeGrip(props: Props) {
  /** 按下那一刻的指针位置，**屏幕坐标**（CSS 像素） */
  let start: { x: number; y: number } | null = null;
  let startRect: Rect | null = null;
  let bounds: DragBounds | null = null;
  /** 相对按下点的位移，由 pointermove 更新。单位是 CSS 像素，发出前才换算成物理像素 */
  let latest: Delta | null = null;
  /** 上一次真正发出去的矩形，用来避免"没变也发" */
  let applied: Rect | null = null;
  let raf = 0;
  let active = false;
  /** 指针真的动过没有 —— 只点一下把手不该产生一次写库 */
  let moved = false;

  const compute = (): Rect | null => {
    if (startRect === null || bounds === null || latest === null) return null;
    return rectFromDrag(
      props.edge,
      startRect,
      toPhysical(latest, window.devicePixelRatio || 1),
      bounds,
    );
  };

  const flush = () => {
    raf = 0;
    if (!active || !moved) return;
    const rect = compute();
    if (rect === null) return;
    if (applied !== null && same(applied, rect)) return;
    applied = rect;
    void ipc.previewPanelRect(rect.x, rect.y, rect.w, rect.h);
  };

  const schedule = () => {
    if (raf === 0) raf = requestAnimationFrame(flush);
  };

  const onPointerMove = (e: PointerEvent) => {
    if (!active || start === null) return;
    // ⚠️ screenX / screenY，不是 clientX / clientY —— 见文件头的第 1 条
    latest = { x: e.screenX - start.x, y: e.screenY - start.y };
    if (!moved && (latest.x !== 0 || latest.y !== 0)) moved = true;
    schedule();
  };

  const stop = () => {
    active = false;
    if (raf !== 0) {
      cancelAnimationFrame(raf);
      raf = 0;
    }
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', onPointerUp);
    window.removeEventListener('pointercancel', onPointerUp);
    document.body.classList.remove('resizing');
  };

  const onPointerUp = () => {
    const rect = active && moved ? compute() : null;
    stop();
    if (rect !== null) {
      // 落库 + 权威套用都在 Rust 侧：它会再夹一次，然后把最终尺寸播回来。
      // 位置和尺寸一次写完（拆两次的话中间那一帧是"新位置 + 旧尺寸"，拖左 / 上边时看得见）。
      void ipc.endResize(rect.x, rect.y, rect.w, rect.h);
    }
    start = null;
    startRect = null;
    bounds = null;
    latest = null;
    applied = null;
    moved = false;
  };

  const onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();

    start = { x: e.screenX, y: e.screenY };
    latest = { x: 0, y: 0 };
    applied = null;
    moved = false;
    active = true;
    document.body.classList.add('resizing');
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp);
    window.addEventListener('pointercancel', onPointerUp);

    try {
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    } catch {
      // 捕获失败只意味着"指针拖出窗口会断"，不该让整个拖拽起不来
    }

    // 起点与上下限只有 Rust 知道（取决于当前显示器的工作区）。取回来之前 `compute()`
    // 返回 null，那几帧空转，拿到之后下一帧就接上 —— 用户感知不到。
    void ipc
      .beginResize()
      .then((b) => {
        if (!active) return;
        startRect = { x: b.x, y: b.y, w: b.w, h: b.h };
        bounds = {
          min: { w: b.min_w, h: b.min_h },
          work: { x: b.work_x, y: b.work_y, w: b.work_w, h: b.work_h },
          margin: b.margin,
        };
        schedule();
      })
      .catch((err) => {
        console.error('[抽屉] 取面板尺寸约束失败', err);
        stop();
      });
  };

  onCleanup(stop);

  return <div class={`grip ${props.class}`} onPointerDown={onPointerDown} />;
}
