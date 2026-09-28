//! 窗口几何、吸附、越界平移、失焦收起与背景模糊（§3.4 / §4.3）。
//!
//! 两条贯穿全文的规则：
//!  1. **展开不改变窗口位置**：锚点 = 折叠条的左上角，面板向右下增长。
//!     只有面板会越出工作区时才做**最小必要平移**（最后一道保险，正常摆放不触发）。
//!  2. **尺寸一次到位，动画只在 CSS 层**：折叠 ↔ 展开只调用一次 `set_size` +
//!     一次 `set_position`，中间不插任何过渡（§3.4 关键规则 3 / 优化清单 #2、#3）。
//!
//! 所有几何计算都收敛在 `anchor` / `snap_top` / `default_position` / `clamp_to_work`
//! 四个**纯函数**里，它们带单测；下面带副作用的函数只是把结果喂给 Tauri。

use serde_json::Value;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

use crate::appstate::{AppState, MARGIN, PANEL_MIN_H, PANEL_MIN_W};

/// 吸附上边缘的触发距离（逻辑像素）。拖到这个距离以内就贴上去。
pub const SNAP_THRESHOLD: i32 = 24;

/// 托盘菜单交互时抑制"失焦收起"的时长（§3.4 关键规则 6）。
///
/// 点托盘图标会让窗口失焦，如果不抑制，用户想点托盘菜单里的「展开」，
/// 结果面板先自己收起来了。
pub const SUPPRESS_MS: i64 = 300;

/// 拖动窗口时抑制"失焦收起"的时长。
///
/// 比 [`SUPPRESS_MS`] 长得多，因为要覆盖的不只是"按下的那一瞬"：
/// 系统模态移动循环起来时窗口会走一遍焦点变化，而一次拖动能持续好几秒。
/// 折叠态下 `Moved` 会持续续期（见 `on_window_event`），所以这个值只需要
/// 足够撑到第一次移动事件到达。
pub const DRAG_SUPPRESS_MS: i64 = 1_500;

/// 工作区（显示器可用区域）。
///
/// 说明：这里用**整个显示器范围**而不是 `work_area()`。
/// 抽屉是 `alwaysOnTop` 且默认贴屏幕右上角，任务栏在下方，两者不冲突；
/// 而 `work_area()` 的可用性随 Tauri 小版本浮动，为它绑一个签名不划算。
/// 真要避开任务栏时，改这里一处即可。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl WorkArea {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

/* ------------------------------ 纯函数：几何 ------------------------------ */

/// 展开时的落点。
///
/// 正常情况返回原点（左上角不动）。只有「原点 + 面板尺寸」越出工作区边距时
/// 才向左/向上平移，而且**只移必要的量**——这样面板不会因为一次展开就跳很远。
pub fn anchor(origin: (i32, i32), panel: (u32, u32), work: &WorkArea) -> (i32, i32) {
    let pw = panel.0 as i32;
    let ph = panel.1 as i32;
    let mut x = origin.0;
    let mut y = origin.1;

    // 右/下越界：往回挪
    let max_x = work.right() - MARGIN - pw;
    if x > max_x {
        x = max_x;
    }
    let max_y = work.bottom() - MARGIN - ph;
    if y > max_y {
        y = max_y;
    }

    // 左/上越界：贴住边距（面板比工作区还大时就贴左上角）
    x = x.max(work.x + MARGIN);
    y = y.max(work.y + MARGIN);

    (x, y)
}

/// 上边缘吸附。
pub fn snap_top(y: i32, work: &WorkArea, enabled: bool) -> i32 {
    if enabled && (y - work.y).abs() <= SNAP_THRESHOLD {
        work.y
    } else {
        y
    }
}

/// 首次启动的默认位置：右上区域（§4.10）。
pub fn default_position(work: &WorkArea, bar_w: u32) -> (i32, i32) {
    (work.right() - MARGIN - bar_w as i32, work.y + MARGIN)
}

/// 把锚点整个收进工作区。拖动结束、改折叠条宽度、换显示器时都要跑一遍。
pub fn clamp_to_work(p: (i32, i32), size: (u32, u32), work: &WorkArea) -> (i32, i32) {
    let max_x = (work.right() - MARGIN - size.0 as i32).max(work.x + MARGIN);
    let max_y = (work.bottom() - MARGIN - size.1 as i32).max(work.y + MARGIN);
    (p.0.clamp(work.x + MARGIN, max_x), p.1.clamp(work.y + MARGIN, max_y))
}

/// 面板尺寸的夹取：**所有要用到面板尺寸的地方都必须过这里**。
///
/// 为什么需要它：面板尺寸是用户可拖的（`Settings.panel_w` / `panel_h`），而"多大算合适"
/// 取决于当前显示器 —— 在一台 3840 宽的屏幕上拖到 1600，换回笔记本的 1366 屏就撑出去了。
/// `clamp_to_work` 只能保住**位置**（把窗口挪回界内），保不住**尺寸**，两件事要分开做。
///
/// 工作区比下限还小时（极端小屏）允许下限 —— 宁可窗口比屏幕大，也不给出一个装不下的面板。
pub fn clamp_panel_size(size: (u32, u32), work: &WorkArea) -> (u32, u32) {
    let max_w = (work.w - 2 * MARGIN).max(PANEL_MIN_W as i32) as u32;
    let max_h = (work.h - 2 * MARGIN).max(PANEL_MIN_H as i32) as u32;
    (size.0.clamp(PANEL_MIN_W, max_w), size.1.clamp(PANEL_MIN_H, max_h))
}

/// 当前形态在这块工作区上**实际生效**的尺寸。
///
/// 折叠条没有可调上限（它只有宽度，且已经夹在 180–420），所以只有展开态需要夹。
fn size_on(state: &Arc<AppState>, expanded: bool, work: &WorkArea) -> (u32, u32) {
    let size = state.size_for(expanded);
    if expanded {
        clamp_panel_size(size, work)
    } else {
        size
    }
}

/* ------------------------------ 副作用 ------------------------------ */

/// 当前显示器的工作区。拿不到就退回主显示器，再拿不到就给一个保守值。
fn work_area_of(win: &WebviewWindow) -> WorkArea {
    if let Ok(Some(m)) = win.current_monitor() {
        let pos = m.position();
        let size = m.size();
        return WorkArea::new(pos.x, pos.y, size.width as i32, size.height as i32);
    }
    if let Ok(Some(m)) = win.primary_monitor() {
        let pos = m.position();
        let size = m.size();
        return WorkArea::new(pos.x, pos.y, size.width as i32, size.height as i32);
    }
    WorkArea::new(0, 0, 1920, 1080)
}

/// 折叠条位置落库（锚点就是它，展开时从这里推面板落点）。
fn persist_anchor(state: &Arc<AppState>, x: i32, y: i32) {
    if let Err(e) = state.db.tx(|c| {
        crate::store::settings::set(c, "window_x", &Value::from(x))?;
        crate::store::settings::set(c, "window_y", &Value::from(y))
    }) {
        tracing::warn!(error = %e, "保存窗口位置失败");
    }
    let mut s = state.settings.write();
    s.window_x = Some(x);
    s.window_y = Some(y);
}

/// 初始化：定位到上次的位置（或默认右上角）、套用置顶与模糊。
pub fn init(app: &AppHandle, state: &Arc<AppState>) {
    let Some(win) = app.get_webview_window("main") else {
        tracing::error!("找不到主窗口，窗口层无法初始化");
        return;
    };

    let settings = state.settings_snapshot();
    let work = work_area_of(&win);
    let (bw, bh) = size_on(state, false, &work);

    let origin = match (settings.window_x, settings.window_y) {
        (Some(x), Some(y)) => (x, y),
        _ => default_position(&work, bw),
    };
    let origin = clamp_to_work(origin, (bw, bh), &work);
    let origin = (origin.0, snap_top(origin.1, &work, settings.snap_top));

    apply_geometry(&win, origin, (bw, bh));
    let _ = win.set_always_on_top(settings.always_on_top);

    if let Err(e) = state.db.tx(|c| {
        crate::store::settings::set(c, "window_x", &Value::from(origin.0))?;
        crate::store::settings::set(c, "window_y", &Value::from(origin.1))
    }) {
        tracing::debug!(error = %e, "首次定位落库失败");
    }

    init_effect(&win);

    let _ = win.show();
    tracing::info!(x = origin.0, y = origin.1, "窗口已就位");
}

fn apply_geometry(win: &WebviewWindow, pos: (i32, i32), size: (u32, u32)) {
    // 顺序：先改尺寸再改位置。反过来的话，窗口在「面板尺寸 + 旧位置」的中间态
    // 会闪一下（尤其在屏幕右边缘）。
    if let Err(e) = win.set_size(PhysicalSize::new(size.0, size.1)) {
        tracing::warn!(error = %e, "设置窗口尺寸失败");
    }
    if let Err(e) = win.set_position(PhysicalPosition::new(pos.0, pos.1)) {
        tracing::warn!(error = %e, "设置窗口位置失败");
    }
}

/// 背景模糊（§4.3）。
///
/// **只能用 `apply_blur`**：官方文档标注 `apply_acrylic` 在 Win10 v1903+ 的拖动/缩放
/// 场景下性能很差，而本项目的窗口会被频繁拖动。
#[cfg(target_os = "windows")]
fn init_effect(win: &WebviewWindow) {
    if let Err(e) = window_vibrancy::apply_blur(win, Some((28, 28, 27, 180))) {
        tracing::warn!(error = %e, "应用背景模糊失败（不影响功能）");
    }
}

#[cfg(not(target_os = "windows"))]
fn init_effect(_win: &WebviewWindow) {}

/// 折叠 ↔ 展开。**幂等**：已经在目标状态时直接返回，不产生任何副作用（§3.4 关键规则 5）。
pub fn set_expanded(app: &AppHandle, state: &Arc<AppState>, expanded: bool) -> anyhow::Result<()> {
    let already = state.expanded.load(Ordering::Relaxed);
    if already == expanded {
        tracing::debug!(expanded, "窗口已处于目标状态，忽略");
        return Ok(());
    }
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| anyhow::anyhow!("找不到主窗口"))?;

    let settings = state.settings_snapshot();
    let work = work_area_of(&win);
    let size = size_on(state, expanded, &work);

    // 先算出落点，再动窗口。展开时 `origin` 就是锚点（折叠条的左上角）。
    let current = win
        .outer_position()
        .map(|p| (p.x, p.y))
        .unwrap_or_else(|_| {
            (
                settings.window_x.unwrap_or(work.right() - MARGIN - size.0 as i32),
                settings.window_y.unwrap_or(work.y + MARGIN),
            )
        });

    let (pos, anchor) = if expanded {
        // 锚点 = 折叠条左上角，面板向右下增长（§3.4 关键规则 1）。
        // 拖过左 / 上边缘时面板相对锚点有一个偏移（panel_dx / panel_dy），要一并带上。
        let base = (current.0 + settings.panel_dx, current.1 + settings.panel_dy);
        (anchor(base, size, &work), current)
    } else {
        // 收起：**回到锚点，而不是"当前位置"**——展开时可能因为越界平移过，
        // 用当前位置会让折叠条越走越偏。
        let stored = match (settings.window_x, settings.window_y) {
            (Some(x), Some(y)) => (x, y),
            _ => current,
        };
        (clamp_to_work(stored, size, &work), stored)
    };

    // 顺序关键：先翻状态位，再动窗口。
    // `set_position` 会触发 `WindowEvent::Moved`，而 Moved 的处理里会持久化锚点；
    // 如果状态位还没翻，展开出来的面板位置会被当成锚点存下来。
    state.expanded.store(expanded, Ordering::Relaxed);
    apply_geometry(&win, pos, size);

    if expanded {
        // 展开不该改变锚点，落一次库让它跟当前显示器对齐
        persist_anchor(state, anchor.0, anchor.1);
    }

    // 几何已落地，把权威形态广播出去。前端只跟随这个事件，
    // 于是"两份 expanded 错开"最多存在一个事件周期，不会再卡住（见 events::WINDOW_STATE）。
    broadcast_window_state(app, state);

    // 提到 info 级：形态切换是排障第一现场，debug 级在默认日志里看不到，
    // 之前"拖动后窗口形体错乱"就是因为没有痕迹而只能靠猜。
    tracing::info!(
        expanded,
        x = pos.0,
        y = pos.1,
        w = size.0,
        h = size.1,
        "窗口状态已切换"
    );
    Ok(())
}

/// 把当前形态播给前端。**幂等**，可以在任何觉得"状态可能漂了"的地方调用。
pub fn broadcast_window_state(app: &AppHandle, state: &Arc<AppState>) {
    let expanded = state.expanded.load(Ordering::Relaxed);
    // 广播的必须是**生效**尺寸（夹过工作区的），不是库里那个原始值 ——
    // 前端与集成测试都拿它当"窗口现在多大"的事实，掺进越界的配置值就是埋雷。
    let (width, height) = match app.get_webview_window("main") {
        Some(win) => size_on(state, expanded, &work_area_of(&win)),
        None => state.size_for(expanded),
    };
    crate::appstate::emit(
        app,
        crate::appstate::events::WINDOW_STATE,
        crate::model::WindowStateDto { expanded, width, height },
    );
}

/// 拖动窗口：**先抑制自动收起，再交给系统拖动**。
///
/// 为什么必须裹这一层：拖动会让窗口失焦（系统模态移动循环开始时焦点会走一遍），
/// 而未锁定时"失焦 = 自动收起"。于是用户在拖动过程中窗口被从展开态缩成折叠态，
/// 系统移动循环随后又把窗口矩形还原成拖动开始时的大小 ——
/// 结果就是"窗口是展开尺寸，里面却只画着折叠条"，正是要修的那个 bug。
pub fn begin_drag(app: &AppHandle, state: &Arc<AppState>) -> anyhow::Result<()> {
    suppress_auto_collapse(state, DRAG_SUPPRESS_MS);
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| anyhow::anyhow!("找不到主窗口"))?;
    win.start_dragging()
        .map_err(|e| anyhow::anyhow!("启动窗口拖动失败：{e}"))
}

/// 抑制「失焦收起」一段时间。托盘菜单交互前调用。
pub fn suppress_auto_collapse(state: &Arc<AppState>, ms: i64) {
    let until = crate::store::now_ms() + ms;
    state.suppress_collapse_until.fetch_max(until, Ordering::Relaxed);
}

pub fn collapse_suppressed(state: &Arc<AppState>) -> bool {
    crate::store::now_ms() < state.suppress_collapse_until.load(Ordering::Relaxed)
}

/// 处理窗口事件：失焦收起、拖动持久化。
///
/// 失焦这条路径刻意**只发事件、不直接改窗口**——先让前端把面板淡出（160ms），
/// 前端再回头调 `collapse_window`。同步写库或同步改尺寸会让淡出动画来不及播。
pub fn on_window_event(state: &Arc<AppState>, app: &AppHandle, event: &tauri::WindowEvent) {
    match event {
        tauri::WindowEvent::Focused(false) => {
            let settings = state.settings_snapshot();
            if settings.locked {
                return;
            }
            if collapse_suppressed(state) {
                tracing::debug!("失焦收起被抑制窗口挡下（多半是在点托盘菜单）");
                return;
            }
            if !state.expanded.load(Ordering::Relaxed) {
                return;
            }
            crate::appstate::emit(app, crate::appstate::events::AUTO_COLLAPSE, ());
        }

        tauri::WindowEvent::Moved(pos) => {
            // 展开态的位置可能是被 `anchor` 平移过的，拿它当锚点会让折叠条一步步往左上爬。
            // 所以下面的处理只在折叠态做。
            if state.expanded.load(Ordering::Relaxed) {
                return;
            }

            // 折叠态下窗口在动 = 用户正拖着折叠条：**移动本身就在抑制自动收起**。
            //
            // 只在折叠态刷新，是因为展开态的 Moved 里混着我们自己 `set_position` 造成的
            // 程序化移动（展开时的越界平移），那时候续期会把该收起的时机一直往后推。
            // 折叠态没有这个问题：收起态下自动收起本来就是空操作（它要求当前是展开态）。
            suppress_auto_collapse(state, SUPPRESS_MS);

            let win = app.get_webview_window("main");
            let Some(win) = win else { return };
            let work = work_area_of(&win);
            let (bw, _) = state.size_for(false);
            let snapped = snap_top(pos.y, &work, state.settings_snapshot().snap_top);
            let clamped = clamp_to_work((pos.x, snapped), (bw, crate::appstate::BAR_H), &work);
            persist_anchor(state, clamped.0, clamped.1);
        }

        _ => {}
    }
}

/// 折叠条宽度变化时需要重算位置（宽度是锚点的一部分）。
pub fn on_bar_width_changed(app: &AppHandle, state: &Arc<AppState>) {
    let Some(win) = app.get_webview_window("main") else { return };
    let work = work_area_of(&win);
    let (bw, bh) = state.size_for(false);
    let settings = state.settings_snapshot();
    let origin = match (settings.window_x, settings.window_y) {
        (Some(x), Some(y)) => (x, y),
        _ => default_position(&work, bw),
    };
    let origin = clamp_to_work(origin, (bw, bh), &work);
    apply_geometry(&win, origin, (bw, bh));
    persist_anchor(state, origin.0, origin.1);
}

/// 面板的落点基准：**锚点**（折叠条左上角）+ `panel_dx / panel_dy` 偏移。
///
/// 展开态的位置可能被上次的越界平移挪过，拿"当前位置"当基准会让面板越拖越往左上爬。
/// 拖拽过程中我们从不落库锚点，所以 `settings.window_x/y` 在这里恒等于真正的折叠条位置。
///
/// 偏移只有拖**左 / 上**边缘时才会非零（那两条边必须让窗口原点跟着走，
/// 否则用户拖上边界却看到下边界在动）。记成"相对锚点的偏移"而不是绝对坐标，
/// 是为了让面板继续跟着折叠条走 —— 用户把折叠条拖到别处，面板跟着过去。
pub fn panel_origin(state: &Arc<AppState>, work: &WorkArea) -> (i32, i32) {
    let settings = state.settings_snapshot();
    let base = match (settings.window_x, settings.window_y) {
        (Some(x), Some(y)) => (x, y),
        _ => default_position(work, state.size_for(false).0),
    };
    (base.0 + settings.panel_dx, base.1 + settings.panel_dy)
}

/// 把一个面板矩形收进工作区：先夹尺寸，再夹位置。拖拽逐帧与落地共用。
///
/// 顺序不能反：位置的合法区间本身依赖尺寸，先定尺寸才能算出位置的上限。
pub fn clamp_rect(rect: (i32, i32, u32, u32), work: &WorkArea) -> ((i32, i32), (u32, u32)) {
    let size = clamp_panel_size((rect.2, rect.3), work);
    let pos = clamp_to_work((rect.0, rect.1), size, work);
    (pos, size)
}

/// 按锚点摆好面板。返回实际落点。
///
/// 位置只做**最小必要平移**（`anchor`）：尺寸本身已经被 `clamp_panel_size` 收进工作区了，
/// 正常情况下这里不会挪动。
fn place_panel(
    win: &WebviewWindow,
    state: &Arc<AppState>,
    work: &WorkArea,
    size: (u32, u32),
) -> (i32, i32) {
    let pos = anchor(panel_origin(state, work), size, work);
    apply_geometry(win, pos, size);
    pos
}

/// 展开面板当前**生效**的尺寸（已按工作区夹过）。前端在开始拖拽前拿它当起点。
pub fn panel_size_now(app: &AppHandle, state: &Arc<AppState>) -> (u32, u32) {
    match app.get_webview_window("main") {
        Some(win) => size_on(state, true, &work_area_of(&win)),
        None => state.size_for(true),
    }
}

/// 面板尺寸的上下限。上限取决于当前显示器，只有这里知道。
///
/// 窗口还没建出来时退回到一个"1920×1080 屏"的保守上限，而不是 0 或 `u32::MAX`：
/// 前者会把设置项夹成下限（不可逆的数据损坏），后者会让设置页画出一个荒谬的滑块量程。
/// 真到套用尺寸时（`size_on`）还会按实际工作区再夹一次。
pub fn panel_size_limits(app: &AppHandle) -> ((u32, u32), (u32, u32)) {
    let min = (PANEL_MIN_W, PANEL_MIN_H);
    let max = match app.get_webview_window("main") {
        Some(win) => clamp_panel_size((u32::MAX, u32::MAX), &work_area_of(&win)),
        None => clamp_panel_size((u32::MAX, u32::MAX), &WorkArea::new(0, 0, 1920, 1080)),
    };
    (min, max)
}

/// 当前面板矩形 + 可拖范围，交给前端当拖拽起点。
///
/// 位置用 `anchor(锚点 + 偏移)` 现算，而不是读窗口当前位置：拖动是"以按下那一刻为准"的
/// 相对位移，起点必须和"再展开一次会得到的那个位置"完全一致，否则松手就会跳一下。
pub fn panel_rect_now(app: &AppHandle, state: &Arc<AppState>) -> crate::model::ResizeBoundsDto {
    let (min, max) = panel_size_limits(app);
    let work = match app.get_webview_window("main") {
        Some(win) => work_area_of(&win),
        None => WorkArea::new(0, 0, 1920, 1080),
    };
    let size = match app.get_webview_window("main") {
        Some(win) => size_on(state, true, &work_area_of(&win)),
        None => state.size_for(true),
    };
    let pos = anchor(panel_origin(state, &work), size, &work);
    crate::model::ResizeBoundsDto {
        x: pos.0,
        y: pos.1,
        w: size.0,
        h: size.1,
        min_w: min.0,
        min_h: min.1,
        max_w: max.0,
        max_h: max.1,
        work_x: work.x,
        work_y: work.y,
        work_w: work.w.max(0) as u32,
        work_h: work.h.max(0) as u32,
        margin: MARGIN,
    }
}

/// 绝对矩形 → 落库用的 `(尺寸, 相对锚点的偏移)`。
///
/// `origin` 传的是 `panel_origin` 的结果，**它本身含旧偏移**；`old` 是那次读取到的旧偏移。
/// 减掉它才是相对真正锚点的偏移 —— 忘掉这一步，面板每拖一次就往右下漂一个偏移量，
/// 而"漂移量正好等于上次的偏移"这种 bug 在单次拖拽里完全看不出来。
pub fn rect_to_setting(
    rect: (i32, i32, u32, u32),
    work: &WorkArea,
    origin: (i32, i32),
    old: (i32, i32),
) -> ((u32, u32), (i32, i32)) {
    let (pos, size) = clamp_rect(rect, work);
    let base = (origin.0 - old.0, origin.1 - old.1);
    (size, (pos.0 - base.0, pos.1 - base.1))
}

/// 拖拽落地时该写进库的一组值：`(panel_w, panel_h, panel_dx, panel_dy)`。
///
/// 前端送来的是**绝对矩形**，而面板位置是"锚点 + 偏移"派生出来的，这里换算一次。
pub fn resolve_panel_rect(
    app: &AppHandle,
    state: &Arc<AppState>,
    rect: (i32, i32, u32, u32),
) -> ((u32, u32), (i32, i32)) {
    let Some(win) = app.get_webview_window("main") else {
        return ((rect.2, rect.3), (0, 0));
    };
    let work = work_area_of(&win);
    let origin = panel_origin(state, &work);
    let s = state.settings_snapshot();
    rect_to_setting(rect, &work, origin, (s.panel_dx, s.panel_dy))
}

/// 拖拽把手过程中的逐帧矩形：**只动窗口，不落库、不广播**。
///
/// 收的是**绝对矩形**（含位置）而不是只有尺寸：拖左 / 上边缘时窗口原点要跟着走，
/// 只给尺寸就只能"改大小、原点不动"，表现为拖上边界、下边界在动。
///
/// ⚠️ 这里有意识地对 §3.4 关键规则 3（"绝不逐帧改窗口尺寸"）开了例外，理由是那条规则的
/// 适用对象不同：它针对的是**动画**——应用自己按时间轴改尺寸，180ms 十来帧，而且同时在动
/// 内容（opacity / translateY），两份开销叠加才会"必卡"。用户拖拽把手是**指针直接驱动**的，
/// 与 Windows 自己的 resize 循环同性质，逐帧跟随才是 1:1 的手感；不跟手反而像是坏的。
/// 代价是 DWM 每帧重算背景模糊 —— 注意**窗口恒等于面板**，所以模糊的色调矩形永远对得上，
/// 视觉上不会出现"整屏糊一层"（那正是"先放大窗口再在 CSS 里缩预览"那条路会踩的坑）。
/// 若实测卡顿，退化方案是"拖拽期间只跟尺寸数字、松手才 `set_size`"，
/// 改动只在 `ui/ResizeGrip.tsx` 里 —— **别在这里加节流**，节流已经在 rAF 那层做过一次了。
///
/// **不落库是硬要求**：拖一次是 60 次/秒的写盘。
pub fn preview_panel_rect(app: &AppHandle, state: &Arc<AppState>, rect: (i32, i32, u32, u32)) {
    if !state.expanded.load(Ordering::Relaxed) {
        return;
    }
    let Some(win) = app.get_webview_window("main") else { return };
    let work = work_area_of(&win);
    let (pos, size) = clamp_rect(rect, &work);
    apply_geometry(&win, pos, size);
    tracing::trace!(x = pos.0, y = pos.1, w = size.0, h = size.1, "拖拽预览");
}

/// 面板尺寸变化（拖拽把手落地 / 设置页滑块）后把窗口摆正。
///
/// 与 [`on_bar_width_changed`] 分开，是因为两态的语义不同：
/// 折叠条只动宽度、动完必须落库（宽度是锚点的一部分）；面板动宽高、而且**只影响展开态**——
/// 收起态的窗口就是折叠条尺寸，用户此刻改面板尺寸不该让折叠条跳一下，等展开时自然用新值。
pub fn on_panel_size_changed(app: &AppHandle, state: &Arc<AppState>) {
    if !state.expanded.load(Ordering::Relaxed) {
        tracing::debug!("收起态下面板尺寸变化不立刻生效，等展开时再用");
        return;
    }
    let Some(win) = app.get_webview_window("main") else { return };
    let work = work_area_of(&win);
    let size = size_on(state, true, &work);
    place_panel(&win, state, &work, size);

    // 面板变大可能顶到屏幕边缘，形态要重新播一遍（前端与窗口几何对齐）
    broadcast_window_state(app, state);
    tracing::info!(w = size.0, h = size.1, "面板尺寸已套用");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> WorkArea {
        WorkArea::new(0, 0, 1920, 1080)
    }

    #[test]
    fn 展开_正常摆放时位置不动() {
        // 在屏幕中上方，展开后 (264+584, 40+500) 仍在界内
        assert_eq!(anchor((200, 40), (584, 500), &work()), (200, 40));
    }

    #[test]
    fn 展开_右边缘只向左挪必要的量() {
        // 折叠条贴右边：x = 1920 - 16 - 264 = 1640
        let (x, y) = anchor((1640, 40), (584, 500), &work());
        assert_eq!(x, 1920 - MARGIN - 584, "刚好贴住右边距");
        assert_eq!(y, 40, "y 不该被牵连");
    }

    #[test]
    fn 展开_下边缘只向上挪必要的量() {
        let (x, y) = anchor((200, 1040), (584, 500), &work());
        assert_eq!(x, 200);
        assert_eq!(y, 1080 - MARGIN - 500);
    }

    #[test]
    fn 展开_右下角同时越界() {
        let (x, y) = anchor((1900, 1070), (584, 500), &work());
        assert_eq!(x, 1920 - MARGIN - 584);
        assert_eq!(y, 1080 - MARGIN - 500);
    }

    #[test]
    fn 展开_原点在工作区外时贴住左上边距() {
        let (x, y) = anchor((-500, -500), (584, 500), &work());
        assert_eq!((x, y), (MARGIN, MARGIN));
    }

    #[test]
    fn 展开_面板比工作区还大时退化到左上角() {
        let w = WorkArea::new(0, 0, 400, 300);
        assert_eq!(anchor((0, 0), (584, 500), &w), (MARGIN, MARGIN));
    }

    #[test]
    fn 展开_多显示器负坐标工作区() {
        // 副屏在主屏左侧，坐标是负的：x ∈ [-1920, 0]
        let w = WorkArea::new(-1920, 0, 1920, 1080);
        // 完全在界内就不动
        assert_eq!(anchor((-1000, 40), (584, 500), &w), (-1000, 40));
        // 贴近右边缘（也就是与主屏的交界处）：只向左挪到刚好贴住边距
        assert_eq!(anchor((-30, 10), (584, 500), &w), (0 - MARGIN - 584, MARGIN));
    }

    #[test]
    fn 吸附_在阈值内贴到上边缘() {
        assert_eq!(snap_top(10, &work(), true), 0);
        assert_eq!(snap_top(-10, &work(), true), 0, "略微越出上边也算要吸附");
        assert_eq!(snap_top(SNAP_THRESHOLD, &work(), true), 0);
        assert_eq!(snap_top(SNAP_THRESHOLD + 1, &work(), true), SNAP_THRESHOLD + 1);
        assert_eq!(snap_top(10, &work(), false), 10, "关掉吸附就不动");
    }

    #[test]
    fn 默认位置_右上角() {
        let (x, y) = default_position(&work(), 264);
        assert_eq!(x, 1920 - MARGIN - 264);
        assert_eq!(y, MARGIN);
    }

    #[test]
    fn 收进工作区_把整条折叠条放进来() {
        assert_eq!(clamp_to_work((5000, 5000), (264, 40), &work()), (1920 - MARGIN - 264, 1080 - MARGIN - 40));
        assert_eq!(clamp_to_work((-99, -99), (264, 40), &work()), (MARGIN, MARGIN));
        assert_eq!(clamp_to_work((800, 500), (264, 40), &work()), (800, 500), "界内不动");
    }

    #[test]
    fn 收进工作区_窗口比工作区大时也不会算出反区间() {
        let w = WorkArea::new(0, 0, 200, 30);
        let p = clamp_to_work((0, 0), (584, 500), &w);
        assert_eq!(p, (MARGIN, MARGIN));
    }

    #[test]
    fn 展开与收起是互逆的() {
        let w = work();
        let origin = (1640, 40);
        let expanded = anchor(origin, (584, 500), &w);
        assert_ne!(expanded, origin, "贴右边时必须平移");
        // 收起时用锚点（而不是展开后的位置）→ 回到原点
        let back = clamp_to_work(origin, (264, 40), &w);
        assert_eq!(back, origin);
    }

    /* ---------- 面板尺寸可配置（拖拽把手 / 设置页） ---------- */

    #[test]
    fn 面板尺寸_界内不动() {
        assert_eq!(clamp_panel_size((700, 600), &work()), (700, 600));
    }

    #[test]
    fn 面板尺寸_低于下限时抬到下限() {
        assert_eq!(
            clamp_panel_size((10, 10), &work()),
            (crate::appstate::PANEL_MIN_W, crate::appstate::PANEL_MIN_H)
        );
    }

    #[test]
    fn 面板尺寸_超出工作区时压到工作区减两侧边距() {
        // 1920 - 2*16 = 1888；1080 - 2*16 = 1048
        assert_eq!(clamp_panel_size((5000, 5000), &work()), (1888, 1048));
    }

    #[test]
    fn 面板尺寸_工作区比下限还小时允许下限() {
        // 极端小屏：宁可窗口比屏幕大，也不给一个装不下面板的结果
        let tiny = WorkArea::new(0, 0, 200, 150);
        assert_eq!(
            clamp_panel_size((500, 500), &tiny),
            (crate::appstate::PANEL_MIN_W, crate::appstate::PANEL_MIN_H)
        );
    }

    #[test]
    fn 面板尺寸_夹取之后无论锚点在哪都留在工作区内() {
        // 这正是上限取 `work.w - 2*MARGIN` 而不是 `work.w` 的理由：
        // anchor() 会在「锚点 + 面板」越界时平移，两侧各留一个 MARGIN，
        // 夹取后的尺寸才不会被锚点的位置顶出屏幕。
        let w = work();
        let size = clamp_panel_size((5000, 5000), &w);
        // 把锚点摆到工作区最右端（能摆放的极限）
        let origin = (w.right() - MARGIN - 264, w.y + MARGIN);
        let (x, y) = anchor(origin, size, &w);
        assert!(x >= w.x + MARGIN);
        assert!(y >= w.y + MARGIN);
        assert!(x + size.0 as i32 <= w.right() - MARGIN);
        assert!(y + size.1 as i32 <= w.bottom() - MARGIN);
    }

    #[test]
    fn 生效尺寸_展开态按工作区夹_收起态不夹() {
        use crate::model::Settings;
        let db = crate::store::Db::open_memory().unwrap();
        let big = Settings { panel_w: 5000, panel_h: 5000, ..Settings::default() };
        let st = crate::appstate::AppState::new(db, big, std::path::PathBuf::from("."));
        assert_eq!(size_on(&st, true, &work()), (1888, 1048), "超大面板要压回工作区");

        // 折叠条宽度本来就夹在 180–420，不该被小屏再压一次
        let tiny = WorkArea::new(0, 0, 300, 200);
        assert_eq!(size_on(&st, false, &tiny), (264, 40));
    }

    /* ---------- 面板拖拽用的矩形换算 ---------- */

    #[test]
    fn 矩形_界内原样通过() {
        assert_eq!(clamp_rect((300, 200, 584, 500), &work()), ((300, 200), (584, 500)));
    }

    #[test]
    fn 矩形_越界只挪位置不缩尺寸() {
        // 整块矩形拖到屏幕外：位置被收回来，尺寸一个像素都不动。
        // 尺寸与位置分开夹是刻意的 —— 混在一起的话，用户把面板推到屏幕边上时
        // 会看到面板莫名变小（"我只是挪一下，怎么缩了"）。
        let (pos, size) = clamp_rect((-500, -500, 584, 500), &work());
        assert_eq!(pos, (MARGIN, MARGIN));
        assert_eq!(size, (584, 500));
    }

    #[test]
    fn 矩形_尺寸超界时按工作区夹且不牵连位置() {
        let (pos, size) = clamp_rect((200, 100, 5000, 5000), &work());
        assert_eq!(size, ((1920 - 2 * MARGIN) as u32, (1080 - 2 * MARGIN) as u32));
        // 尺寸一旦撑满工作区，位置就没有余量了，必然被收到左上角 —— 这不是 bug，
        // 而是"面板比工作区大"时唯一自洽的结果。
        assert_eq!(pos, (MARGIN, MARGIN));
    }

    #[test]
    fn 偏移换算_必须减掉旧偏移否则每拖一次漂一次() {
        // 锚点 (100, 50) + 旧偏移 (40, 20) → panel_origin 给出的就是 (140, 70)
        let origin = (140, 70);
        let (_, ok) = rect_to_setting((500, 300, 584, 500), &work(), origin, (40, 20));
        assert_eq!(ok, (400, 250), "相对锚点 (100, 50) 的偏移");

        // 反例：把含旧偏移的 origin 直接当锚点 —— 算出来的偏移小了整整一个旧偏移，
        // 于是下次展开时面板往左上跳 (40, 20)，而且每拖一次累积一次。
        let (_, bad) = rect_to_setting((500, 300, 584, 500), &work(), origin, (0, 0));
        assert_eq!(ok.0 - bad.0, 40);
        assert_eq!(ok.1 - bad.1, 20);
    }

    #[test]
    fn 面板原点_无偏移时就是锚点本身() {
        use crate::model::Settings;
        let db = crate::store::Db::open_memory().unwrap();
        let s = Settings { window_x: Some(1640), window_y: Some(16), ..Settings::default() };
        let st = crate::appstate::AppState::new(db, s, std::path::PathBuf::from("."));
        assert_eq!(panel_origin(&st, &work()), (1640, 16), "默认 0 偏移 = 改动前的行为");
    }

    #[test]
    fn 面板原点_偏移叠在锚点上_折叠条一动面板跟着走() {
        use crate::model::Settings;
        let db = crate::store::Db::open_memory().unwrap();
        let s = Settings {
            window_x: Some(100),
            window_y: Some(50),
            panel_dx: 40,
            panel_dy: 20,
            ..Settings::default()
        };
        let st = crate::appstate::AppState::new(db, s, std::path::PathBuf::from("."));
        assert_eq!(panel_origin(&st, &work()), (140, 70));
    }

    /* ---------- 拖动期间抑制自动收起（bug：拖动后窗口形体错乱） ---------- */

    fn state() -> std::sync::Arc<crate::appstate::AppState> {
        use crate::model::Settings;
        let db = crate::store::Db::open_memory().unwrap();
        crate::appstate::AppState::new(db, Settings::default(), std::path::PathBuf::from("."))
    }

    #[test]
    fn 抑制期内的自动收起会被挡下() {
        let st = state();
        assert!(!collapse_suppressed(&st), "初始不该被抑制");
        suppress_auto_collapse(&st, DRAG_SUPPRESS_MS);
        assert!(collapse_suppressed(&st));
    }

    #[test]
    fn 抑制是取最大值而不是覆盖() {
        let st = state();
        suppress_auto_collapse(&st, DRAG_SUPPRESS_MS);
        // 后到的短抑制不能把已经排好的长抑制缩短 ——
        // 否则"拖动中顺手点了一下托盘"会让窗口在拖动中途被收起来。
        suppress_auto_collapse(&st, 1);
        assert!(collapse_suppressed(&st));
    }

    #[test]
    fn 拖动抑制必须明显长于托盘抑制() {
        // 托盘抑制只要挡住"点图标那一下"，拖动抑制要覆盖整段"按下的瞬间 + 焦点变化"。
        // 两者搞反了，拖动开始时窗口会被自己收起来。
        assert!(
            DRAG_SUPPRESS_MS > SUPPRESS_MS * 3,
            "拖动抑制 {DRAG_SUPPRESS_MS}ms 太短，挡不住拖动起始时的失焦"
        );
    }
}
