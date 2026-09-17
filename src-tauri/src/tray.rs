//! 托盘图标与菜单（FR-10）。
//!
//! 为什么托盘是**硬需求**而不是锦上添花：`resizable:false` + `skipTaskbar:true` +
//! `decorations:false` 三条合起来意味着窗口没有任何系统提供的关闭/退出入口，
//! 也不出现在 Alt+Tab 里（§3.4 关键规则 4、§4.3 补充说明）。
//!
//! 菜单项与动作的映射抽成了纯函数 `action_for`，好处是"菜单文案改了但动作接错了"
//! 这类 bug 能在单测里被抓到，而不必真的去点托盘。

use std::sync::Arc;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

use crate::appstate::{emit, events, AppState};
use crate::model::Peer;
use crate::store::{conversation as conv_store, mute as mute_store};
use crate::window;

/// 托盘图标 id。多托盘场景下用它定位（本项目只有一个）。
pub const TRAY_ID: &str = "main";

/// 菜单项 id。与 `action_for` 必须一一对应。
pub mod item {
    pub const TOGGLE: &str = "tray.toggle";
    pub const LOCK: &str = "tray.lock";
    pub const SETTINGS: &str = "tray.settings";
    pub const CACHE: &str = "tray.cache";
    pub const QUIT: &str = "tray.quit";
}

/// 菜单动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    /// 显示 / 收起
    TogglePanel,
    /// 锁定展开（锁定后失焦不收起）
    ToggleLock,
    OpenSettings,
    /// 「清空缓存」实际上打开缓存管理页 —— 直接删数据太粗暴，
    /// 用户需要先看到"要删掉多少、删哪些"
    OpenCacheManager,
    Quit,
    /// 未知 id（菜单被改过、或未来新增项忘了接线）
    Ignore,
}

pub fn action_for(id: &str) -> TrayAction {
    match id {
        item::TOGGLE => TrayAction::TogglePanel,
        item::LOCK => TrayAction::ToggleLock,
        item::SETTINGS => TrayAction::OpenSettings,
        item::CACHE => TrayAction::OpenCacheManager,
        item::QUIT => TrayAction::Quit,
        _ => TrayAction::Ignore,
    }
}

/// 建托盘。
pub fn build(app: &AppHandle, state: &Arc<AppState>) -> tauri::Result<()> {
    let locked = state.settings_snapshot().locked;

    let toggle = MenuItem::with_id(app, item::TOGGLE, "显示 / 收起", true, None::<&str>)?;
    let lock = CheckMenuItem::with_id(app, item::LOCK, "锁定展开", true, locked, None::<&str>)?;
    let settings = MenuItem::with_id(app, item::SETTINGS, "设置…", true, None::<&str>)?;
    let cache = MenuItem::with_id(app, item::CACHE, "缓存管理…", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, item::QUIT, "退出", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&toggle, &lock, &settings, &sep, &cache, &quit])?;

    let icon = match app.default_window_icon().cloned() {
        Some(icon) => icon,
        None => {
            // 图标理论上一定在（tauri.conf.json 的 bundle.icon 第一项就是它），
            // 但托盘没有图标会直接 build 失败，所以给一个程序化生成的兜底单色方块。
            tracing::warn!("默认窗口图标缺失，使用兜底托盘图标");
            let rgba = fallback_icon_rgba();
            tauri::image::Image::new_owned(rgba, ICON_SIZE, ICON_SIZE)
        }
    };

    let state_click = state.clone();
    let state_menu = state.clone();
    let lock_item = lock.clone();

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("QQ 抽屉")
        .menu(&menu)
        // 左键单击直接弹菜单：这是本应用唯一的「入口」，
        // 藏成右键菜单会让用户找不到退出在哪
        .show_menu_on_left_click(true)
        .on_tray_icon_event(move |_tray, event| {
            // 点托盘会让窗口失焦。先把"失焦收起"关掉 300ms，
            // 否则用户想点菜单里的「显示 / 收起」，面板会先自己收起来（§3.4 关键规则 6）。
            if matches!(event, TrayIconEvent::Click { .. }) {
                window::suppress_auto_collapse(&state_click, window::SUPPRESS_MS);
            }
        })
        .on_menu_event(move |app, event| {
            let id: &str = event.id().as_ref();
            handle(app, &state_menu, action_for(id), &lock_item);
        })
        .build(app)?;

    Ok(())
}

/// 兜底托盘图标尺寸。
pub const ICON_SIZE: u32 = 32;

/// 程序化生成的兜底图标：32×32 的琥珀色圆角方块（§3.6 的强调色）。
///
/// 返回 RGBA 字节流，长度必须是 `ICON_SIZE * ICON_SIZE * 4`。
pub fn fallback_icon_rgba() -> Vec<u8> {
    const AMBER: [u8; 3] = [0xD8, 0xA6, 0x4A];
    let n = ICON_SIZE as i32;
    let mut out = Vec::with_capacity((n * n * 4) as usize);
    // 圆角半径 6px，中心 2px 用透明，让它在浅色/深色任务栏上都看得清
    let r = 6i32;
    for y in 0..n {
        for x in 0..n {
            let dx = (r - x).max(x - (n - 1 - r)).max(0);
            let dy = (r - y).max(y - (n - 1 - r)).max(0);
            let outside = dx * dx + dy * dy > r * r;
            let inset = x >= 4 && x <= n - 5 && y >= 4 && y <= n - 5;
            if outside {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else if inset {
                out.extend_from_slice(&[AMBER[0], AMBER[1], AMBER[2], 255]);
            } else {
                out.extend_from_slice(&[AMBER[0] / 2, AMBER[1] / 2, AMBER[2] / 2, 255]);
            }
        }
    }
    out
}

/// 同步「锁定展开」的勾选态。设置页改了锁定之后也要调它，否则托盘勾选会和实际状态不一致。
pub fn sync_lock_check(app: &AppHandle, locked: bool) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let Some(menu) = tray.menu() else { return };
    let Some(item) = menu.get(item::LOCK) else { return };
    let Some(check) = item.as_check_menuitem() else { return };
    if let Err(e) = check.set_checked(locked) {
        tracing::debug!(error = %e, "同步托盘勾选态失败");
    }
}

fn handle(app: &AppHandle, state: &Arc<AppState>, action: TrayAction, lock_item: &CheckMenuItem<tauri::Wry>) {
    // 每个动作都要再抑制一次：菜单展开是异步的，点击时 300ms 可能已经过去
    window::suppress_auto_collapse(state, window::SUPPRESS_MS);

    match action {
        TrayAction::TogglePanel => {
            // 只发意图，几何交给前端那一条路径（它要先播 160ms 淡出再让 Rust 改尺寸）。
            // 从托盘直接改尺寸会让动画没机会播，而且会出现两条互相打架的尺寸写入。
            emit(app, events::TOGGLE_PANEL, ());
        }

        TrayAction::ToggleLock => {
            let now = !state.settings_snapshot().locked;
            if let Err(e) = state.db.tx(|c| {
                crate::store::settings::set(c, "locked", &serde_json::Value::Bool(now))
            }) {
                tracing::warn!(error = %e, "保存锁定状态失败");
                return;
            }
            state.settings.write().locked = now;
            let _ = lock_item.set_checked(now);
            crate::appstate::toast(
                app,
                if now {
                    "已锁定展开（点外部不再收起）"
                } else {
                    "已取消锁定"
                },
                "info",
            );
        }

        TrayAction::OpenSettings => open_sheet(app, state, "settings"),
        TrayAction::OpenCacheManager => open_sheet(app, state, "cache"),

        TrayAction::Quit => {
            tracing::info!("从托盘退出");
            app.exit(0);
        }

        TrayAction::Ignore => tracing::debug!("未接线的托盘菜单项被点击"),
    }
}

/// 「设置 / 缓存管理」都在面板里的浮层上。托盘点击时面板可能还是收起的，
/// 所以要先展开——但**不能**让前端去调 `expand_window`（那样会绕过淡出动画），
/// 这里的顺序是：发 `toggle_panel` 让前端展开 → 再发 `open_sheet` 让前端开浮层。
fn open_sheet(app: &AppHandle, state: &Arc<AppState>, sheet: &str) {
    if !state.expanded.load(std::sync::atomic::Ordering::Relaxed) {
        emit(app, events::TOGGLE_PANEL, ());
    }
    emit(app, events::OPEN_SHEET, sheet.to_string());
}

/// 全局快捷键的「静音当前会话」动作（`hotkey.rs` 与托盘共用这一份实现）。
///
/// 没有正在查看的会话时退到折叠条上显示的那一条 —— 快捷键按下去什么都没发生
/// 是最让人困惑的反馈。
pub fn toggle_mute_current(app: &AppHandle, state: &Arc<AppState>) {
    let target = *state.viewing.read();
    let peer = match target {
        Some(p) => p,
        None => {
            let list = state.db.with(conv_store::list).unwrap_or_default();
            match crate::sched::notify::pick_bar(&list) {
                Some(c) => Peer { peer_type: c.peer_type, peer_id: c.peer_id },
                None => {
                    crate::appstate::toast(app, "还没有会话可以静音", "info");
                    return;
                }
            }
        }
    };

    let muted = state
        .db
        .with(|c| mute_store::is_muted(c, peer))
        .unwrap_or(false);
    let next = !muted;

    if let Err(e) = state
        .db
        .tx(|c| mute_store::set(c, peer, next, Some("用户手动切换")))
    {
        tracing::warn!(error = %e, "切换静音失败");
        return;
    }

    let name = state
        .db
        .with(|c| conv_store::get(c, peer))
        .ok()
        .flatten()
        .map(|c| c.name)
        .unwrap_or_else(|| peer.key());
    crate::appstate::toast(
        app,
        if next {
            format!("已静音「{name}」")
        } else {
            format!("已取消静音「{name}」")
        },
        "info",
    );

    crate::appstate::emit_conversations(app, state);
}

#[cfg(test)]
#[allow(uncommon_codepoints)]
mod tests {
    use super::*;

    #[test]
    fn 每个菜单项都有动作() {
        assert_eq!(action_for(item::TOGGLE), TrayAction::TogglePanel);
        assert_eq!(action_for(item::LOCK), TrayAction::ToggleLock);
        assert_eq!(action_for(item::SETTINGS), TrayAction::OpenSettings);
        assert_eq!(action_for(item::CACHE), TrayAction::OpenCacheManager);
        assert_eq!(action_for(item::QUIT), TrayAction::Quit);
    }

    #[test]
    fn 未知菜单项不 panic() {
        assert_eq!(action_for(""), TrayAction::Ignore);
        assert_eq!(action_for("tray.不存在的项"), TrayAction::Ignore);
    }

    #[test]
    fn 菜单项_id_不重复() {
        let ids = [item::TOGGLE, item::LOCK, item::SETTINGS, item::CACHE, item::QUIT];
        let mut sorted = ids.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "菜单 id 重复会让两个项触发同一个动作");
    }

    #[test]
    fn 兜底图标尺寸与字节数对得上() {
        let rgba = fallback_icon_rgba();
        assert_eq!(rgba.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        // 四角透明、中心不透明
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0], "左上角应该是透明的圆角");
        let center = ((ICON_SIZE / 2 * ICON_SIZE + ICON_SIZE / 2) * 4) as usize;
        assert_eq!(rgba[center + 3], 255, "中心必须不透明");
    }
}
