/**
 * 消息列表（FR-19 / FR-20 / NFR-05）。
 *
 * **必须虚拟滚动**：不虚拟化时加载 500 条历史直接把内存拉起来，100 MB 的预算守不住
 * （优化清单 #1）。这里只渲染可视区 ±10 行，行高按需测量。
 *
 * ## 视口位置由锚点决定，不由 scrollTop 决定
 *
 * `scrollTop` 是绝对像素值，但它所在的内容坐标系 `offsets` 会变：行高从估值换成
 * 实测、向上翻页插入历史，都会改写它。坐标系一变，同一个 `scrollTop` 就指向别的内容
 * 了 —— 用户看到的就是"滚着滚着消息自己往回退"（bug 3）。
 *
 * 所以每次坐标系换代，都要先用**旧**坐标系取出锚点（哪一行 + 行内偏移），再用**新**
 * 坐标系反算 `scrollTop`。见 `core/vscroll.ts` 的 `anchorKeyAt` / `topForKey`，以及
 * 决定"这一轮到底要不要动视口"的 `planViewport`。
 *
 * ## 四条硬约束（都踩过，改之前先读）
 *
 * ① **行高估值只学一次**（`learnEstimate`）。它参与 `padTop` / `padBottom`，每来一个
 *    样本就重算的话**所有未渲染行**会一起浮动 —— 500 行的列表里是几千像素的
 *    `scrollHeight` 突变，滚动条抽风、渲染窗口边界乱走、引发新一波测量，自激。
 * ② **只有「向下追加」才跟随底部，翻页期间一律不跟**。否则往上翻历史会被拽回底部。
 * ③ **程序补偿写入的 `scrollTop` 必须和用户的滚动区分开**。浏览器派发的 scroll 事件
 *    里分不出这两者，不区分的话补偿会被当成"用户滚到底"，重新打开贴底跟随。
 * ④ **逻辑位置（`scrollTop` 信号）与真实 `scrollTop` 必须同批更新**，且 `offsets`
 *    必须与 DOM 布局一致。任一条不成立，`range` 就会用"新坐标系 + 旧位置"算出一段
 *    根本不在视口附近的行 —— 表现就是"快速滚动会飞到很上面的聊天记录，过一会又滚回来"。
 *    落地方式：`applyTop` 是唯一写入口（写完顺手同步逻辑位置），`measureRendered`
 *    在每次换代时同步量一遍真实行高（读 `offsetHeight` 会强制 layout，但只在这一处读）。
 */

import { For, Show, createEffect, createMemo, createSignal, onCleanup } from 'solid-js';
import * as S from '../state/store';
import * as ipc from '../state/ipc';
import type { MessageRow as Row } from '../core/grouping';
import { MessageRow } from './MessageRow';
import {
  ESTIMATE_SAMPLE_MIN,
  ESTIMATED_ROW_HEIGHT,
  alignToAnchor,
  anchorFromTops,
  anchorKeyAt,
  buildOffsets,
  computeRange,
  estimateRowHeight,
  isAppendOnly,
  mergeMeasured,
  planViewport,
  shouldLoadMore,
  topForKey,
  type AnchorKey,
} from '../core/vscroll';
import { showMenu } from './ContextMenu';
import { loadOlder } from '../state/events';

const OVERSCAN = 10;
/**
 * 距底部多少像素以内算「贴底」。收得很紧，是因为它同时决定"行高回填要不要把视口
 * 拉到底部"：阈值一松，用户往上滚个二三十像素仍被算作贴底，紧接着的回填就把他
 * 拽回最底部（bug 3 的"向上滚动有时弹回最底部"）。
 */
const PINNED_SLACK = 12;
/** 观察目标攒到这么多就先清一遍已滚出渲染区的，别让 ResizeObserver 的列表无限膨胀 */
const OBSERVE_PRUNE_AT = 200;
/**
 * 一次视口校正最多迭代几轮。
 * 每轮都可能因为视口移动而把新的一批行拉进渲染窗口，那批行量完又会让坐标系更准；
 * 真实布局下两轮就收敛，给三轮是留余量，免得病态布局下无限打转。
 */
const MAX_SETTLE_PASSES = 3;

export function MessageList() {
  let scroller: HTMLDivElement | undefined;

  /** 逻辑视口位置：`range` 的唯一输入，与 `offsets` 同批更新（约束 ④） */
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewportH, setViewportH] = createSignal(0);
  /** `ref` 回调跑完、`scroller` 拿到真节点。它同时是「首屏可以开始定位」的信号 */
  const [ready, setReady] = createSignal(false);
  /** key → 实测高度；没测到的先用 `estimate()` */
  const [heights, setHeights] = createSignal<Record<string, number>>({});
  /** 是否贴底——只有贴底时才跟随新消息 */
  const [pinned, setPinned] = createSignal(true);

  /** 未渲染行的高度估值，**只学一次**（约束 ①），换会话才重学 */
  const [estimate, setEstimate] = createSignal<number>(ESTIMATED_ROW_HEIGHT);
  let estimateLocked = false;

  /** 翻页进行中：不重复触发翻页（但贴底态照常重算，见 `onScroll`） */
  let paging = false;
  /**
   * 我们最后一次主动写入、并被浏览器接受的那个 `scrollTop`。
   * 用来在 scroll 事件里分辨「用户在滚」与「我们在补偿」——两者必须区别对待，
   * 否则补偿会被当成用户滚到底部，重新打开贴底跟随（约束 ③）。
   */
  let expectTop: number | null = null;

  const rows = S.currentRows;
  const rowKeys = createMemo(() => rows().map((r) => r.key));

  const offsets = createMemo(() => {
    const h = heights();
    const est = estimate();
    return buildOffsets(rows().map((r) => h[r.key] ?? est));
  });

  const win = createMemo(() => computeRange(scrollTop(), viewportH(), offsets(), OVERSCAN));
  /**
   * 补偿期间把渲染窗口从列表顶端铺满。**只在前插历史时短暂开启**：那一批新行的
   * 真实行高必须一次量到，否则剩下的估值会在之后被逐个换成实测值，总高度一变再变
   * —— 用户看到的就是滚动条闪、长短不一（bug 3 第一条反馈）。
   */
  const [wide, setWide] = createSignal(false);
  const view = createMemo(() => {
    const w = win();
    return wide() ? { start: 0, end: w.end } : w;
  });
  const range = createMemo<Row[]>(() => rows().slice(view().start, view().end));
  const padTop = createMemo(() => offsets()[view().start] ?? 0);
  const padBottom = createMemo(() =>
    Math.max(0, (offsets()[rows().length] ?? 0) - (offsets()[view().end] ?? 0)),
  );

  /**
   * **唯一**的滚动位置写入口。
   *
   * 两件事必须一起做，缺一个都会让渲染窗口画到别处去（约束 ④）：
   * ① 写真实 `scrollTop`，并记下浏览器夹取后的**真实值** —— 我们想要的位置可能
   *    超出可滚动范围（总高度刚变小的时候很常见），记想要的那个值就等于让逻辑
   *    位置从此与实际错开。
   * ② 把逻辑位置跟上。`range` 由它派生，慢一帧的话，这一帧渲染的就是「新坐标系 +
   *    旧位置」算出来的、完全不在视口附近的行。
   *
   * 返回「位置真的动了没有」：补偿要迭代到不动为止，靠它判收敛（比猜轮数可靠）。
   */
  const writeTop = (el: HTMLDivElement, top: number): boolean => {
    const before = el.scrollTop;
    const target = Math.round(top);
    let moved = false;
    // 幂等：和当前值相同就别写。写了浏览器照样派发 scroll 事件，和用户的滚动打架。
    if (Math.abs(before - target) >= 1) {
      el.scrollTop = target;
      // 值真的动了才记 —— 目标越界被夹回原值时不派发 scroll 事件，记下去就成了一笔
      // 永远对不上的账，之后用户在附近滚一下会被误判成"我们自己写的"给吞掉。
      if (el.scrollTop !== before) {
        expectTop = el.scrollTop;
        moved = true;
      }
    }
    if (scrollTop() !== el.scrollTop) setScrollTop(el.scrollTop);
    return moved;
  };

  /* --------------------- 视口的真值：DOM（不是估值） --------------------- */

  /**
   * 容器内容区的顶边在视口坐标里的位置。
   * `.msgs` 没有边框，所以 border-box 顶边就是 padding-box 顶边，可以直接当基准。
   */
  const contentBase = (el: HTMLDivElement): number => el.getBoundingClientRect().top;

  /**
   * 视口顶边落在**哪一行**、行内多少像素。
   *
   * 这是整套视口逻辑里唯一不依赖任何估值的量：行就在眼前，位置直接量。
   * 原来的做法是拿「累计高度表 + scrollTop」反推 —— 那张表里混着未渲染行的**估值**，
   * 真机上（图片行 184px vs 文字行 30px）估值与实测能差一大截，反推出来的锚点落在
   * 别的行上，于是"翻一页就跳到很上边的聊天记录"。量 DOM 则完全没有这个环节。
   *
   * 取「顶边在容器顶之上、且最靠下的那一行」＝视口顶边所落的那一行。
   */
  const anchorFromDom = (el: HTMLDivElement): AnchorKey | null => {
    const base = contentBase(el);
    const tops: { key: string; top: number }[] = [];
    for (const node of el.querySelectorAll<HTMLElement>('.row-wrap')) {
      const key = node.dataset.key;
      if (key === undefined || key === '') continue;
      tops.push({ key, top: node.getBoundingClientRect().top - base });
    }
    return anchorFromTops(tops);
  };

  /** 某一行此刻在视口坐标里的顶边偏移；它不在渲染窗口里时返回 null */
  const rowOffset = (el: HTMLDivElement, key: string): number | null => {
    const base = contentBase(el);
    for (const node of el.querySelectorAll<HTMLElement>('.row-wrap')) {
      if (node.dataset.key === key) return node.getBoundingClientRect().top - base;
    }
    return null;
  };

  /* ------------------------------ 行高测量 ------------------------------ */

  const observed = new Set<HTMLElement>();

  /** 清掉已经滚出渲染区的观察目标（ResizeObserver 不会自己松手，不清就是内存泄漏） */
  const prune = () => {
    for (const el of observed) {
      if (!el.isConnected) {
        observer.unobserve(el);
        observed.delete(el);
      }
    }
  };

  const observeRow = (el: HTMLDivElement, key: string) => {
    el.dataset.key = key;
    if (observed.size > OBSERVE_PRUNE_AT) prune();
    observer.observe(el);
    observed.add(el);
  };

  /**
   * 首屏拿到第一批样本就把估值钉死（约束 ①）。
   *
   * 取样只取**当前列表**里的行：`heights` 里可能还留着上个会话的 key，拿它们算中位数
   * 会让这批样本失真。
   */
  const learnEstimate = (h: Record<string, number>) => {
    if (estimateLocked) return;
    const sample: number[] = [];
    for (const r of rows()) {
      const v = h[r.key];
      if (v !== undefined) sample.push(v);
    }
    if (sample.length < ESTIMATE_SAMPLE_MIN) return;
    estimateLocked = true;
    const est = estimateRowHeight(sample);
    if (est !== estimate()) setEstimate(est);
  };

  /**
   * 同步量一遍渲染窗口里每一行的真实高度。
   *
   * 读 `offsetHeight` 会强制同步 layout —— 所以只在这里读，每轮最多一次。换来的是
   * `offsets` 与 DOM 布局**立刻**一致：没有这一步的话，新渲染出来的行在
   * `ResizeObserver` 回调到达之前都还按估值参与计算，用这种坐标系反算出来的锚点位置
   * 本身就是错的（翻页插入一页两三百像素的偏高行时，偏差能到几百像素 —— 用户看到的是
   * "载入新历史时闪一下、衔接不准"）。
   *
   * 返回值是「有没有量到新高度」：没有就说明坐标系已经收敛，可以停止迭代。
   */
  const measureRendered = (el: HTMLDivElement): boolean => {
    const measured: { key: string; height: number }[] = [];
    for (const node of el.querySelectorAll<HTMLElement>('.row-wrap')) {
      const key = node.dataset.key;
      if (key === undefined || key === '') continue;
      measured.push({ key, height: node.offsetHeight });
    }
    const merged = mergeMeasured(heights(), measured);
    if (!merged.changed) return false;
    setHeights(merged.heights);
    learnEstimate(merged.heights);
    return true;
  };

  /**
   * 用户此刻**正看着哪一行的第几像素**。
   *
   * 这是整个视口逻辑里唯一跨坐标系稳定的量：行高回填、向上翻页，`offsets` 会整个换代，
   * 同一个 `scrollTop` 指向别的内容；而"哪一行 + 行内偏移"始终说的是同一处内容。
   * 所以位置的真值是它，`scrollTop` 只是它在当前坐标系下的投影。
   */
  let anchor: AnchorKey | null = null;

  /** 已经排了一次补偿微任务，同一批里后面的请求合并掉 */
  let restoreQueued = false;
  /** 这批补偿要先把渲染窗口从列表顶端铺满（前插历史时用，见 `settle`） */
  let widePending = false;

  /**
   * 取「视口顶边落在哪一行、行内偏移多少」。
   *
   * **优先量 DOM**，量不到（那一行还没进渲染窗口）才退回坐标系反推。
   * 前者是真实布局，后者混着估值 —— 真机上行高差异大（图片 184px vs 文字 30px），
   * 估值反推出来的行号可以是错的，那就是"翻页后落到别的消息上"。
   */
  const captureAnchor = (el: HTMLDivElement): AnchorKey | null =>
    anchorFromDom(el) ?? anchorKeyAt(offsets(), rowKeys(), el.scrollTop);

  /**
   * 把视口钉回锚点。**唯一的补偿入口**。
   *
   * ## 为什么必须是「量 DOM」，而不是「按累计高度表算」
   *
   * 高度表里混着未渲染行的估值：翻页插入的一页里，只有落进渲染窗口的那部分量到了
   * 真实高度，其余仍是估值（真机上短句 30px、图片行 184px，差一个数量级）。按这种表
   * 反算出来的锚点位置本身就是错的 —— 用户看到的就是"翻一页，视口落到很上边的记录"。
   *
   * 而锚点行**一定在视口附近**，也就是一定在渲染窗口里，它的真实位置随时能量。
   * 所以补偿改成两步：先用坐标系把锚点行拉进窗口（粗定位，允许不准），
   * 再读它的 DOM 真值把位置钉死（精定位，误差归零）。任何一层估值出问题都不会
   * 传导到最终位置 —— 这是「闭环」而不是「开环」。
   *
   * ## 为什么前插一整批时要先把窗口铺满
   *
   * 只量渲染窗口盖住的那部分，剩下那些行的估值会在之后被陆续换成实测值，每换一次
   * 总高度就变一次 —— 那就是"滚动条一直闪、长短不一"。前插时把窗口从列表顶端铺到
   * 锚点之后，整批一次量准，总高度当场定下来。
   */
  const settle = (el: HTMLDivElement, a: AnchorKey) => {
    let coarseDone = false;
    for (let pass = 0; pass < MAX_SETTLE_PASSES; pass += 1) {
      const measured = measureRendered(el);
      const off = rowOffset(el, a.key);
      let moved = false;
      if (off !== null) {
        // 精定位：这一行现在在视口里偏移 `off`，要让它回到"锚点内偏移"处
        moved = writeTop(el, alignToAnchor(el.scrollTop, off, a.inner));
      } else if (!coarseDone) {
        // 粗定位：锚点行还没进窗口，先用坐标系把它拉过来
        const top = topForKey(offsets(), rowKeys(), a.key, a.inner);
        if (top !== null) moved = writeTop(el, top);
        coarseDone = true;
      }
      // 位置不动、也没有新测量 → 收敛
      if (!moved && !measured) break;
    }
  };

  const restore = (el: HTMLDivElement, prepended: boolean) => {
    const a = anchor;
    if (a === null) return;
    if (prepended) {
      setWide(true);
      settle(el, a);
      // 收窄回正常窗口后会重新布局，得再钉一次位置
      setWide(false);
    }
    settle(el, a);
    // 落点已定，把锚点重新对齐到**实际**位置，下次换代拿它当基准才是准的
    anchor = anchorFromDom(el) ?? a;
  };

  /**
   * 排一次补偿。合并同一批里的重复请求——`offsets` 换代常常连着好几次
   * （测量、回填各来一轮），每轮都单独补一次会互相打架。
   */
  const scheduleRestore = (prepended: boolean) => {
    // 前插一整批时要先铺满窗口量准，取「或」——同一批里只要有一次是前插就得铺
    widePending ||= prepended;
    if (restoreQueued) return;
    restoreQueued = true;
    queueMicrotask(() => {
      restoreQueued = false;
      const p = widePending;
      widePending = false;
      if (scroller !== undefined) restore(scroller, p);
    });
  };

  const handleResize = (entries: ResizeObserverEntry[]) => {
    const measured: { key: string; height: number }[] = [];
    for (const e of entries) {
      const el = e.target as HTMLElement;
      if (el === scroller) {
        // 容器自己变了（面板展开 / 收起 / 窗口缩放）→ 视口高度跟着变
        setViewportH(el.clientHeight);
        continue;
      }
      if (!el.isConnected) {
        observer.unobserve(el);
        observed.delete(el);
        continue;
      }
      const key = el.dataset.key;
      if (key === undefined || key === '') continue;
      measured.push({ key, height: el.offsetHeight });
    }
    // 高度变了就交给下面那个统一入口去决定视口怎么走 —— 这里不做补偿，
    // 补偿只允许有一个地方做，否则两份判定会互相推翻。
    const merged = mergeMeasured(heights(), measured);
    if (merged.changed) {
      setHeights(merged.heights);
      learnEstimate(merged.heights);
    }
  };

  // 观察者必须在这里就建好，不能等 `onMount`：JSX 的 `ref` 回调比 `onMount` 先跑，
  // 等 onMount 再建的话**首屏渲染出来的行一个都不会被观察**，高度一直停在估值上，
  // 直到用户开始滚动才补测 —— 那就是「一滚就大幅抖动」。
  const observer = new ResizeObserver(handleResize);
  onCleanup(() => {
    observer.disconnect();
    observed.clear();
  });

  /* --------------------- 视口跟随（唯一的判定入口） --------------------- */

  let lastPeer: string | null = null;
  /** 上一轮的坐标系（行序 + 高度表），用来判定「换代」 */
  let lastKeys: string[] | null = null;
  let lastOffsets: number[] | null = null;
  /** 刚换过会话、还没跳过底 */
  let jumpPending = false;

  /**
   * 所有「坐标系换代」都从这里过：换会话、行高回填、向上翻页插入历史。
   *
   * 判定本身在 `core/vscroll.ts` 的 `planViewport`（有单测），这里只负责执行。
   *
   * ⚠️ 这个 effect **刻意不读 `scrollTop`**。读它的话，用户每滚一下 effect 就重跑一次，
   * 而重跑时会重新走一遍"要不要校正视口"的判定 —— 视口校正和用户的滚动就会互相打架
   * （bug 3 的滚轮失灵、卡在中间都源于此）。用户滚动只需要更新锚点，那件事在 `onScroll`
   * 里做，不必惊动这个 effect。
   */
  createEffect(() => {
    // `scroller` 是普通变量，没有响应性。首屏那一轮 effect 跑在 JSX 之前，那时它还是
    // undefined —— 必须等 `ref` 回调把它置上再重跑一次，否则首屏永远不会定位。
    if (!ready()) return;

    const peer = S.state.current;
    const keys = rowKeys();
    const offs = offsets();
    const el = scroller;
    if (el === undefined) return;

    const peerChanged = peer !== lastPeer;
    const prevKeys = lastKeys;
    const prevOffsets = lastOffsets;
    // 坐标系是否换代用**引用比较**：`offsets` / `rowKeys` 都是 memo，没变就返回同一个
    // 数组。用户滚动只改 `scrollTop`、不动这两个引用 —— 所以"用户滚了一下"不会被误判
    // 成换代、也就不会被"校正"回原处。
    const coordChanged =
      peerChanged || prevOffsets === null || offs !== prevOffsets || keys !== prevKeys;

    if (peerChanged) {
      lastPeer = peer;
      jumpPending = true;
      setPinned(true); // 换会话默认从最新看起
      // 行高分布是按会话的（有人话密有人爱发图），换会话就重新学一次估值。
      // 顺带把实测缓存丢掉：滑动窗口只有 30 行，重测很便宜，留着反而会跨会话膨胀。
      estimateLocked = false;
      setHeights({});
      setEstimate(ESTIMATED_ROW_HEIGHT);
      expectTop = null; // 上个会话的补偿记录对新会话没有意义
      anchor = null; // 旧锚点指向旧会话的行，留着会在两批不相干的消息之间搬位置
    }

    lastKeys = keys;
    lastOffsets = offs;

    if (keys.length === 0) return;

    // 行序是不是真的变了：只有长度或首行变了才算。消息内容更新（撤回、发送状态、
    // 图片解码完）会让 `rowKeys` 换一个新数组，但行还是那几行 —— 那种情况按「行高变了」
    // 处理即可，别误判成"往前面插了东西"。
    const orderChanged =
      prevKeys === null || keys.length !== prevKeys.length || keys[0] !== prevKeys[0];
    const appended = orderChanged && isAppendOnly(prevKeys ?? [], keys);
    // 往前面插了一整批历史（翻页）。这种换代要把整批量准，见 `restore`。
    const prepended = orderChanged && !appended && !peerChanged;

    const action = planViewport({
      peerChanged,
      jumpPending,
      coordChanged,
      orderChanged,
      appended,
      pinned: pinned(),
    });
    if (action === 'hold') return;

    jumpPending = false;

    if (action === 'jump-bottom') {
      // 此刻图片多半还没解码，撑高之后坐标系会再变一轮，那一轮继续跟
      anchor = null;
      writeTop(el, el.scrollHeight);
      anchor = anchorFromDom(el);
      return;
    }

    // 钉锚点交给微任务去做 —— 只有在那个上下文里，写信号才能同步推进 DOM，
    // 「量 → 修 → 再量」的闭环才真的收敛（见 `settle`）
    scheduleRestore(prepended);
  });

  /* ------------------------------ 滚动与翻页 ------------------------------ */

  const onScroll = () => {
    const el = scroller;
    if (!el) return;

    const v = el.scrollTop;
    setScrollTop(v);

    // 补偿派生出来的那次 scroll 事件不代表用户意图，跳过（约束 ③）。
    // 只认"我们刚写下去的那一个值"：其它一律当用户在滚 —— 宁可多算一次贴底态，
    // 也不能把用户的滚动误吞掉（吞掉的后果是滚轮时灵时不灵）。
    const mine = expectTop !== null && Math.abs(v - expectTop) < 1;
    expectTop = null;
    if (mine) return;

    setPinned(v + el.clientHeight >= el.scrollHeight - PINNED_SLACK);
    // 用户滚到哪，锚点就跟到哪 —— 这是"用户在看哪一行"的唯一来源。此刻没有测量或
    // 插入夹在中间，`offsets` 与 `scrollTop` 同代，取出来的位置是准的。
    anchor = captureAnchor(el);
    // 翻页等待期间不再重复触发翻页，但**贴底态照常重算**：用户在这几十~几百毫秒里
    // 又滚回底部时，贴底跟随必须能恢复，否则他会觉得"到底了也不跟新消息"。
    if (!paging && shouldLoadMore(v)) void paginateUp();
  };

  /**
   * 向上翻页（FR-20）。
   *
   * 这里**不做位置补偿**：`loadOlder` 一改行序，上面那个统一入口就会排一次补偿，用
   * 「用户翻页前正看着的那一行」把视口钉回去。两条补偿路径互相推翻过一次（翻页那条
   * 拿的是翻页前的锚点快照，和用户在这期间滚出来的位置打架），所以只保留一条。
   *
   * 这里只负责一个闸：别再重复发请求。
   */
  const paginateUp = async () => {
    if (scroller === undefined || paging) return;
    paging = true;
    setPinned(false); // 用户在看历史，明确退出贴底
    try {
      await loadOlder();
    } finally {
      paging = false;
    }
  };

  const jumpTo = (messageId: string) => {
    const target = scroller?.querySelector<HTMLElement>(`[data-mid="${messageId}"]`);
    if (target && scroller) {
      target.scrollIntoView({ block: 'center' });
      setScrollTop(scroller.scrollTop);
      // 跳转也是一次"用户意图"，锚点必须跟着走，否则之后的行高回填会把他拽回原处
      anchor = captureAnchor(scroller);
      target.animate?.(
        [
          { outline: '1px solid rgba(239,159,39,.9)' },
          { outline: '1px solid rgba(239,159,39,0)' },
        ],
        { duration: 900 },
      );
      return;
    }
    S.showToast('这条消息不在当前加载的范围内');
  };

  const onBlankMenu = (e: MouseEvent) => {
    showMenu(e, [
      { label: '标记全部已读', action: () => void ipc.markAllRead() },
      {
        label: '刷新历史',
        action: () => {
          const conv = S.currentConversation();
          if (conv) void ipc.refreshConversations();
        },
      },
      { label: '设置', action: () => S.setSheet('settings') },
    ]);
  };

  return (
    <div
      class="msgs"
      ref={(el) => {
        scroller = el;
        observer.observe(el); // 容器尺寸变化（面板展开 / 收起 / 窗口缩放）也要跟上
        setViewportH(el.clientHeight);
        setReady(true); // 让视口定位那一个 effect 可以开跑
      }}
      onScroll={onScroll}
      onContextMenu={onBlankMenu}
    >
      <Show when={S.currentAtTop()}>
        <div class="top-hint">以上是全部</div>
      </Show>
      <Show when={S.state.historyLoading}>
        <div class="loading-hint">正在加载更早的消息…</div>
      </Show>

      <div class="vr-pad" style={{ height: `${padTop()}px` }} />

      <For each={range()}>
        {(row) => (
          <div
            class="row-wrap"
            ref={(el) => observeRow(el, row.key)}
            data-mid={row.msg.message_id}
          >
            <MessageRow row={row} selfId={S.state.selfId} onJump={jumpTo} />
          </div>
        )}
      </For>

      <div class="vr-pad" style={{ height: `${padBottom()}px` }} />

      <Show when={rows().length === 0}>
        <div class="empty-hint">
          {S.state.conn === 'connected' ? '还没有消息' : '未连接到 NapCat，点击重试'}
        </div>
      </Show>
    </div>
  );
}
