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

use crate::appstate::{AppState, MARGIN};

/// 吸附上边缘的触发距离（逻辑像素）。拖到这个距离以内就贴上去。
pub const SNAP_THRESHOLD: i32 = 24;

/// 托盘菜单交互时抑制"失焦收起"的时长（§3.4 关键规则 6）。
///
/// 点托盘图标会让窗口失焦，如果不抑制，用户想点托盘菜单里的「展开」，
/// 结果面板先自己收起来了。
pub const SUPPRESS_MS: i64 = 300;

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
    let (bw, bh) = state.size_for(false);
    let work = work_area_of(&win);

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
    let size = state.size_for(expanded);

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
        // 锚点 = 折叠条左上角，面板向右下增长（§3.4 关键规则 1）
        (anchor(current, size, &work), current)
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

    tracing::debug!(expanded, x = pos.0, y = pos.1, "窗口状态已切换");
    Ok(())
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
            // 只在折叠态记录锚点：展开态的位置可能是被 `anchor` 平移过的，
            // 拿它当锚点会让折叠条一步步往左上爬。
            if state.expanded.load(Ordering::Relaxed) {
                return;
            }
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

#[cfg(test)]
#[allow(uncommon_codepoints)]
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
}
