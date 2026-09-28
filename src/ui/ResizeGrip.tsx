/**
 * 面板尺寸把手（优化 2）。
 *
 * 三个把手共享这一个组件：右边缘只改宽、下边缘只手改高、右下角两个都改。
 * 之所以要三个而不是一个角标 —— 用角标只想调宽的时候，高度也会跟着动，手感是坏的。
 *
 * **只放在右 / 下两侧**：左、上的把手会移动窗口原点，而"锚点 = 折叠条左上角、
 * 面板向右下增长"是 §3.4 关键规则 1，破了它整个几何模型都要重做。
 *
 * 三条实现要点：
 *  1. **用位移（delta）而不是指针绝对坐标**：窗口本身在跟着动，用绝对坐标当尺寸会
 *     自我反馈成抖动。位移只依赖按下那一刻的原点，天然稳定。
 *  2. **一帧最多一次 IPC**（rAF 合并）：指针事件的频率远高于帧率，不合并就是在给
 *     IPC 灌垃圾；而 `preview_panel_size` 本来就是"一帧 set_size 一次窗口"，
 *     也只能跟上帧率。
 *  3. **指针捕获**：往内拖时窗口会缩小到指针之外，没有捕获就收不到 pointermove，
 *     拖拽会中途"冻住"。
 *
 * ⚠️ 这里逐帧改窗口尺寸，是对 §3.4 关键规则 3 的**有意例外**，理由写在
 * `window::preview_panel_size` 的注释里（那条规则针对的是应用自己驱动的时间轴动画）。
 * 要退化成"松手才落地"，改这里就够：mousemove 里别发 `previewPanelSize`，
 * 只在 `pointerup` 里发 `endResize`。**别在 Rust 侧加节流**，这里已经按帧合并过了。
 */

import { onCleanup } from 'solid-js';
import * as ipc from '../state/ipc';
import {
  sizeFromDrag,
  toPhysical,
  type Delta,
  type Limits,
  type ResizeAxis,
  type Size,
} from '../core/resize';

interface Props {
  axis: ResizeAxis;
  class: string;
}

export function ResizeGrip(props: Props) {
  /** 按下那一刻的指针位置（CSS 像素） */
  let start: { x: number; y: number } | null = null;
  let startSize: Size | null = null;
  let limits: Limits | null = null;
  /** 相对按下点的位移，由 pointermove 更新 */
  let latest: Delta | null = null;
  /** 上一次真正发出去的尺寸，用来避免"没变也发" */
  let applied: Size | null = null;
  let raf = 0;
  let active = false;

  const compute = (): Size | null => {
    if (startSize === null || limits === null || latest === null) return null;
    // clientX/Y 是 CSS 像素，而 Rust 的 set_size 收的是物理像素：
    // 不乘 devicePixelRatio，在 150% 缩放的屏上拖起来只有行程的 2/3。
    return sizeFromDrag(
      props.axis,
      startSize,
      toPhysical(latest, window.devicePixelRatio || 1),
      limits,
    );
  };

  const flush = () => {
    raf = 0;
    if (!active) return;
    const size = compute();
    if (size === null) return;
    if (applied !== null && applied.w === size.w && applied.h === size.h) return;
    applied = size;
    void ipc.previewPanelSize(size.w, size.h);
  };

  const schedule = () => {
    if (raf === 0) raf = requestAnimationFrame(flush);
  };

  const onPointerMove = (e: PointerEvent) => {
    if (!active || start === null) return;
    latest = { x: e.clientX - start.x, y: e.clientY - start.y };
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
    const size = active ? compute() : null;
    stop();
    // `applied === null` = 指针压根没动过（只是点了一下把手）：
    // 这种情况不该产生一次写库。
    if (size !== null && applied !== null) {
      // 落库 + 权威套用都在 Rust 侧：它会再夹一次，然后把最终尺寸播回来
      void ipc.endResize(size.w, size.h);
    }
    start = null;
    startSize = null;
    limits = null;
    latest = null;
    applied = null;
  };

  const onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();

    start = { x: e.clientX, y: e.clientY };
    latest = { x: 0, y: 0 };
    applied = null;
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

    // 上下限只有 Rust 知道（取决于当前显示器的工作区）。取回来之前 `compute()` 会返回
    // null，那几帧空转，拿到之后下一帧就接上 —— 用户感知不到。
    void ipc
      .beginResize()
      .then((b) => {
        if (!active) return;
        startSize = { w: b.w, h: b.h };
        limits = {
          min: { w: b.min_w, h: b.min_h },
          max: { w: b.max_w, h: b.max_h },
        };
        schedule();
      })
      .catch((err) => {
        console.error('[抽屉] 取面板尺寸上下限失败', err);
        stop();
      });
  };

  onCleanup(stop);

  return (
    <div
      class={`grip ${props.class}`}
      onPointerDown={onPointerDown}
      title={props.axis === 'both' ? '拖动调整面板大小' : undefined}
    />
  );
}
