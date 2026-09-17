//! 全局快捷键（FR-10 / §3.9）。
//!
//! 同样是硬需求：`skipTaskbar` 生效后窗口从 Alt+Tab 消失（§5 踩坑 #3），
//! 快捷键是唯一"随时能把抽屉叫出来"的手段。
//!
//! 默认：`Ctrl+Alt+Q` 展开/收起，`Ctrl+Alt+M` 静音当前会话。
//!
//! 这里最值得测的是 `normalize` —— 它是用户可编辑的配置项（设置页里手输），
//! 一个拼错的字符串不该让整个注册流程静默失败。

use std::sync::Arc;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::appstate::{emit, events, AppState};

/// 快捷键动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyKind {
    /// 展开 / 收起（必需）
    Toggle,
    /// 静音 / 取消静音当前会话（可选）
    Mute,
}

/// 一个快捷键定义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub kind: HotkeyKind,
    pub accelerator: &'static str,
}

/// 规范的修饰键写法。
const MODS: [(&str, &str); 6] = [
    ("ctrl", "Ctrl"),
    ("control", "Ctrl"),
    ("alt", "Alt"),
    ("shift", "Shift"),
    ("super", "Super"),
    ("meta", "Super"),
];

/// 修饰键的固定排列顺序。
///
/// 用户写 `alt+ctrl+q` 和 `ctrl+alt+q` 应当落库成同一个字符串，
/// 否则"看起来一样的两个组合"会在设置页里显示成两副样子。
const MOD_ORDER: [&str; 4] = ["Ctrl", "Alt", "Shift", "Super"];

/// 校验并规范化快捷键字符串：`"ctrl + alt + q"` → `"Ctrl+Alt+Q"`。
///
/// 规则（故意收紧，因为这是用户手输的）：
///  · 必须恰好一个非修饰键；
///  · 至少一个修饰键 —— 裸 `Q` 当全局快捷键会吞掉所有输入；
///  · 主键只允许 A–Z / 0–9 / F1–F24 / 少量具名键。
///
/// 返回 `None` 表示这个字符串不可用。
pub fn normalize(accel: &str) -> Option<String> {
    let parts: Vec<String> = accel
        .split('+')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 2 {
        return None;
    }

    let mut mods: Vec<&'static str> = Vec::new();
    let mut keys: Vec<String> = Vec::new();

    for p in &parts {
        let lower = p.to_ascii_lowercase();
        match MODS.iter().find(|(k, _)| *k == lower) {
            Some((_, canon)) => {
                if !mods.contains(canon) {
                    mods.push(canon);
                }
            }
            None => keys.push(canonical_key(p)?),
        }
    }

    if mods.is_empty() || keys.len() != 1 {
        return None;
    }

    mods.sort_by_key(|m| MOD_ORDER.iter().position(|x| x == m).unwrap_or(usize::MAX));
    Some(format!("{}+{}", mods.join("+"), keys[0]))
}

fn canonical_key(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }

    // F1..F24
    let lower = s.to_ascii_lowercase();
    if let Some(num) = lower.strip_prefix('f') {
        if let Ok(n) = num.parse::<u8>() {
            if (1..=24).contains(&n) {
                return Some(format!("F{n}"));
            }
            return None;
        }
    }

    if s.len() == 1 {
        let c = s.chars().next()?;
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase().to_string());
        }
    }

    // 具名键：只放常用的几个，其余一律拒绝（拼错了就该被拒绝，而不是变成不生效的注册）
    let named = match lower.as_str() {
        "space" => Some("Space"),
        "enter" | "return" => Some("Enter"),
        "tab" => Some("Tab"),
        "backquote" | "`" => Some("Backquote"),
        "escape" | "esc" => Some("Escape"),
        "minus" | "-" => Some("Minus"),
        "equal" | "=" => Some("Equal"),
        "comma" | "," => Some("Comma"),
        "period" | "." => Some("Period"),
        "slash" | "/" => Some("Slash"),
        "semicolon" | ";" => Some("Semicolon"),
        "quote" | "'" => Some("Quote"),
        "bracketleft" | "[" => Some("BracketLeft"),
        "bracketright" | "]" => Some("BracketRight"),
        "backslash" | "\\" => Some("Backslash"),
        _ => None,
    };
    named.map(|s| s.to_string())
}

/// 注册（或重新注册）全部快捷键。
///
/// 先 `unregister_all` 再注册：设置页改完快捷键会再调一次，
/// 不先清干净的话旧组合还留着，用户会以为"没生效"。
pub fn register(app: &AppHandle, state: &Arc<AppState>) -> anyhow::Result<()> {
    let settings = state.settings_snapshot();
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let mut ok = 0usize;
    for (kind, accel) in [
        (HotkeyKind::Toggle, settings.hotkey_toggle.as_str()),
        (HotkeyKind::Mute, settings.hotkey_mute.as_str()),
    ] {
        if accel.trim().is_empty() {
            continue;
        }
        match bind_one(app, state, kind, accel) {
            Ok(()) => {
                ok += 1;
                tracing::info!(kind = ?kind, accel, "快捷键已注册");
            }
            Err(e) => {
                // 单个失败不影响另一个：默认组合可能与别的软件冲突，
                // 这时用户还能用托盘，只是少一个快捷方式
                tracing::warn!(kind = ?kind, accel, error = %e, "快捷键注册失败");
                crate::appstate::toast(app, &format!("快捷键 {accel} 注册失败，可能被占用"), "error");
            }
        }
    }

    if ok == 0 {
        tracing::warn!("没有任何全局快捷键注册成功，请检查设置");
    }
    Ok(())
}

fn bind_one(
    app: &AppHandle,
    state: &Arc<AppState>,
    kind: HotkeyKind,
    raw: &str,
) -> anyhow::Result<()> {
    let canon = normalize(raw).ok_or_else(|| {
        anyhow::anyhow!("快捷键格式不对：{raw}（需要至少一个修饰键加一个主键，如 Ctrl+Alt+Q）")
    })?;
    let shortcut: Shortcut = canon
        .parse()
        .map_err(|e| anyhow::anyhow!("无法解析快捷键 {canon}: {e}"))?;

    let state_for_handler = state.clone();
    app.global_shortcut().on_shortcut(
        shortcut,
        move |app, _sc, event| {
            // 只在按下时触发。不判这个的话一次按键会触发按下与抬起两次，
            // 表现就是"按一下展开又立刻收起"。
            if event.state() != ShortcutState::Pressed {
                return;
            }
            match kind {
                HotkeyKind::Toggle => emit(app, events::TOGGLE_PANEL, ()),
                HotkeyKind::Mute => crate::tray::toggle_mute_current(app, &state_for_handler),
            }
        },
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 规范化_默认组合() {
        assert_eq!(normalize("Ctrl+Alt+Q").as_deref(), Some("Ctrl+Alt+Q"));
        assert_eq!(normalize("ctrl+alt+q").as_deref(), Some("Ctrl+Alt+Q"));
        assert_eq!(normalize("  ctrl + alt + q  ").as_deref(), Some("Ctrl+Alt+Q"));
        assert_eq!(normalize("CTRL+ALT+M").as_deref(), Some("Ctrl+Alt+M"));
    }

    #[test]
    fn 规范化_修饰键顺序被统一() {
        assert_eq!(normalize("alt+ctrl+q").as_deref(), Some("Ctrl+Alt+Q"));
        assert_eq!(normalize("shift+alt+q"), normalize("alt+shift+q"));
        assert_eq!(normalize("super+q").as_deref(), Some("Super+Q"));
        assert_eq!(normalize("meta+q").as_deref(), Some("Super+Q"));
        assert_eq!(normalize("control+q").as_deref(), Some("Ctrl+Q"));
    }

    #[test]
    fn 规范化_重复修饰键被折叠() {
        assert_eq!(normalize("ctrl+ctrl+alt+q").as_deref(), Some("Ctrl+Alt+Q"));
    }

    #[test]
    fn 规范化_功能键与数字() {
        assert_eq!(normalize("ctrl+f1").as_deref(), Some("Ctrl+F1"));
        assert_eq!(normalize("shift+f24").as_deref(), Some("Shift+F24"));
        assert_eq!(normalize("ctrl+1").as_deref(), Some("Ctrl+1"));
    }

    #[test]
    fn 规范化_具名键() {
        assert_eq!(normalize("ctrl+space").as_deref(), Some("Ctrl+Space"));
        assert_eq!(normalize("ctrl+`").as_deref(), Some("Ctrl+Backquote"));
        assert_eq!(normalize("alt+enter").as_deref(), Some("Alt+Enter"));
        assert_eq!(normalize("alt+esc").as_deref(), Some("Alt+Escape"));
    }

    #[test]
    fn 拒绝_没有修饰键() {
        assert!(normalize("q").is_none(), "裸键当全局快捷键会吞掉所有输入");
        assert!(normalize("f1").is_none());
        assert!(normalize("space").is_none());
    }

    #[test]
    fn 拒绝_没有主键或主键太多() {
        assert!(normalize("ctrl+alt").is_none());
        assert!(normalize("ctrl+q+w").is_none());
        assert!(normalize("ctrl+alt+shift").is_none());
    }

    #[test]
    fn 拒绝_空串与垃圾输入() {
        assert!(normalize("").is_none());
        assert!(normalize("   ").is_none());
        assert!(normalize("ctrl+随便").is_none(), "认不出的键名必须被拒绝");
        assert!(normalize("ctrl+f25").is_none(), "F25 不存在");
        assert!(normalize("ctrl+f0").is_none());
    }

    #[test]
    fn 拒绝_主键超过一个字符且不是具名键() {
        assert!(normalize("ctrl+ab").is_none());
        assert!(normalize("ctrl+你好").is_none());
    }

    #[test]
    fn 规范化结果能被解析成_Shortcut() {
        // 规范化之后的字符串必须真的能被插件解析 —— 否则注册会静默失败。
        // 这里只用字母与功能键：具名键的拼写在插件各版本间有过出入，不拿它当契约。
        for raw in ["ctrl+alt+q", "ctrl+alt+m", "shift+f1", "ctrl+1", "super+q"] {
            let canon = normalize(raw).expect(raw);
            assert!(
                canon.parse::<Shortcut>().is_ok(),
                "{canon} 应该能被解析成 Shortcut"
            );
        }
    }

    #[test]
    fn 规范化是幂等的() {
        for raw in ["ctrl+alt+q", "alt+shift+f5", "super+space"] {
            let once = normalize(raw).unwrap();
            assert_eq!(normalize(&once).as_deref(), Some(once.as_str()));
        }
    }
}
