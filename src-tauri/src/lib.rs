//! 应用装配（§4.2）。
//!
//! 启动顺序是有讲究的，改之前先读这一段：
//!
//! ```text
//!  1. 定位数据目录（%LOCALAPPDATA%\qq-drawer）→ 建目录
//!  2. 打开数据库 + 迁移；读设置
//!  3. 初始化日志（级别来自设置，所以必须在 2 之后）
//!  4. 清空上次运行留下的未读痕迹（FR-39）
//!  5. 建 AppState 并 manage 进 Tauri（此后命令与自定义协议才能拿到它）
//!  6. 窗口定位 → 托盘 → 全局快捷键
//!  7. 起 WS 常驻任务与缓存清理任务
//! ```
//!
//! 两个容易踩的点：
//!  · `manage` 必须在任何"通过 AppHandle 取 state"的代码之前 —— 包括媒体协议回调；
//!  · 日志晚一点初始化没关系（前面几步不打日志），但**必须**在设置读出来之后，
//!    否则 `log_level` 设置形同虚设。

mod appstate;
mod cmd;
mod hotkey;
mod media;
mod model;
mod ob;
mod sched;
mod store;
mod tray;
mod window;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Manager};

use crate::appstate::AppState;
use crate::model::ConnInfo;
use crate::store::{conversation as conv_store, schema, settings as settings_store, Db};

/// 数据目录名。`%LOCALAPPDATA%\<这个名字>`，前端与媒体协议都按它拼路径，改一处不行。
const APP_DIR: &str = "qq-drawer";

/// 日志保留天数（§4.10 高级设置：7 天）。
const LOG_KEEP_DAYS: u64 = 7;

/// 缓存后台清理的间隔（§4.8 #11：每 6 小时）。
const CACHE_SWEEP_HOURS: u64 = 6;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // 图片走自定义协议，**绝不请求外链**（§4.8 #6）。
        // WebView2 里这个协议表现为 `http://media.localhost/<路径>`。
        .register_uri_scheme_protocol(media::MEDIA_SCHEME, |ctx, request| {
            let Some(state) = ctx.app_handle().try_state::<Arc<AppState>>() else {
                return reply(StatusCode::SERVICE_UNAVAILABLE, "text/plain; charset=utf-8", Vec::new());
            };
            let root = state.media_root.clone();
            let path = request.uri().path().to_string();
            if request.method() != tauri::http::Method::GET {
                return reply(StatusCode::METHOD_NOT_ALLOWED, "text/plain; charset=utf-8", Vec::new());
            }
            let served = media::serve(&root, &path);
            let code = StatusCode::from_u16(served.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            reply(code, served.mime, served.body)
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let state = bootstrap(&handle)?;
            let _ = app.manage(state.clone());

            window::init(&handle, &state);
            if let Err(e) = tray::build(&handle, &state) {
                tracing::error!(error = %e, "托盘初始化失败（窗口将没有退出入口）");
            }
            if let Err(e) = hotkey::register(&handle, &state) {
                tracing::warn!(error = %e, "注册全局快捷键失败");
            }

            // WS 常驻任务：断线自己退避重连，所以这里只管起一次
            {
                let h = handle.clone();
                let st = state.clone();
                tauri::async_runtime::spawn(async move { ob::ws::run(h, st).await });
            }

            spawn_cache_sweeper(&handle, &state);

            tracing::info!("QQ 抽屉已启动");
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            let app = window.app_handle().clone();
            let Some(state) = app.try_state::<Arc<AppState>>() else {
                return;
            };
            let state = state.inner().clone();
            window::on_window_event(&state, &app, event);
        })
        .invoke_handler(tauri::generate_handler![
            cmd::conn_status,
            cmd::retry_connect,
            cmd::list_conversations,
            cmd::refresh_conversations,
            cmd::load_messages,
            cmd::send_message,
            cmd::retry_send,
            cmd::mark_read,
            cmd::mark_all_read,
            cmd::set_mute,
            cmd::delete_message,
            cmd::recall_message,
            cmd::add_tab,
            cmd::remove_tab,
            cmd::reorder_tabs,
            cmd::list_members,
            cmd::cache_overview,
            cmd::clear_cache,
            cmd::get_settings,
            cmd::set_setting,
            cmd::save_pasted_image,
            cmd::expand_window,
            cmd::collapse_window,
            cmd::set_viewing,
            cmd::exit_app,
        ])
        .run(tauri::generate_context!())
        .expect("运行 QQ 抽屉失败");
}

fn reply(code: StatusCode, mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .header(tauri::http::header::CONTENT_TYPE, mime)
        // 图片是内容寻址的（文件名就是哈希），所以可以永久缓存
        .header(tauri::http::header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/* ------------------------------ 启动 ------------------------------ */

/// 数据目录：`%LOCALAPPDATA%\qq-drawer`。
///
/// 用 `local_data_dir()` 而不是 `app_local_data_dir()`：后者会带上 identifier
/// （`local.qqdrawer.app`），而规格书与 §4.8 的媒体路径约定都是 `%LOCALAPPDATA%\qq-drawer`。
fn data_root(app: &AppHandle) -> anyhow::Result<PathBuf> {
    let base = app
        .path()
        .local_data_dir()
        .map_err(|e| anyhow::anyhow!("定位本地数据目录失败: {e}"))?;
    Ok(base.join(APP_DIR))
}

fn bootstrap(app: &AppHandle) -> anyhow::Result<Arc<AppState>> {
    let root = data_root(app)?;
    std::fs::create_dir_all(&root)?;

    let db = Db::open(&root.join("data.db"))?;
    let settings = db.with(settings_store::get_all)?;

    init_tracing(&root, &settings.log_level);
    tracing::info!(dir = %root.display(), "数据目录已就位");

    // FR-39：未读痕迹仅在本次运行期间有效 —— 用户很可能已经在手机上读过了
    match db.tx(|c| conv_store::reset_all_unread(c)) {
        Ok(n) if n > 0 => tracing::info!(count = n, "启动时清零未读痕迹"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "清零未读痕迹失败"),
    }

    let state = AppState::new(db, settings, root);

    // self_id 从上一次运行里恢复：还没连上 NapCat 的这段时间里，
    // 界面也得能正确判断"哪些是我发的"（否则历史消息会全部显示成别人发的）。
    if let Ok(Some(raw)) = state
        .db
        .with(|c| schema::meta_get(c, "self_id"))
    {
        if let Ok(id) = raw.parse::<i64>() {
            state.self_id.store(id, std::sync::atomic::Ordering::Relaxed);
            state.set_conn(ConnInfo::disconnected(Some(id)));
            tracing::debug!(self_id = id, "沿用上次的 self_id");
        }
    }

    Ok(state)
}

/// 每 6 小时按 LRU 清一次图片；开了「启动时清理」就再立刻跑一次（§4.8 #11）。
fn spawn_cache_sweeper(app: &AppHandle, state: &Arc<AppState>) {
    let s = state.settings_snapshot();
    if s.cache_clean_on_start {
        let report = media::auto_cleanup(state, s.cache_keep_days, s.cache_limit_bytes);
        if !report.is_empty() {
            tracing::info!(removed = report.removed, freed = report.freed_bytes, "启动清理完成");
        }
    }

    let app = app.clone();
    let state = state.clone();
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(CACHE_SWEEP_HOURS * 3600));
        // 第一次 tick 会立刻返回；启动清理已经单独做过了
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let s = state.settings_snapshot();
            let report = media::auto_cleanup(&state, s.cache_keep_days, s.cache_limit_bytes);
            if !report.is_empty() {
                tracing::info!(removed = report.removed, freed = report.freed_bytes, "定时清理完成");
                crate::appstate::emit_conversations(&app, &state);
            }
        }
    });
}

/* ------------------------------ 日志 ------------------------------ */

/// 日志过期判定（纯函数，便于单测）。
pub fn log_expired(modified_ms: i64, now_ms: i64, keep_days: u64) -> bool {
    now_ms.saturating_sub(modified_ms) > (keep_days as i64) * 86_400_000
}

/// 日志滚动目录：`%LOCALAPPDATA%\qq-drawer\logs`。
///
/// ⚠️ **NFR-12：日志不记录消息正文。** 只记 id、计数、耗时、错误原因。
/// 加日志的时候请守住这条 —— 这是隐私承诺，不是风格偏好。
fn init_tracing(root: &Path, level: &str) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let logs = root.join("logs");
    let _ = std::fs::create_dir_all(&logs);
    prune_logs(&logs, LOG_KEEP_DAYS);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));

    let file = tracing_appender::rolling::daily(&logs, "qq-drawer");
    let (writer, guard) = tracing_appender::non_blocking(file);
    // 官方对这种场景的建议就是泄漏 guard：它的生命周期与本进程一致，
    // 一旦被 drop，后台刷盘线程就停了，最后一批日志会丢。
    std::mem::forget(guard);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(writer);

    #[cfg(debug_assertions)]
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout));

    #[cfg(not(debug_assertions))]
    let registry = tracing_subscriber::registry().with(filter).with(file_layer);

    let _ = registry.try_init();
}

/// 删掉超过保留天数的日志文件。失败只当没发生（日志清理不该影响启动）。
fn prune_logs(dir: &Path, keep_days: u64) {
    let now = store::now_ms();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("qq-drawer") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        let modified_ms = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(now);
        if log_expired(modified_ms, now, keep_days) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    #[test]
    fn 日志_超过保留天数就该删() {
        let now = 1_700_000_000_000;
        assert!(log_expired(now - 8 * DAY, now, 7));
        assert!(!log_expired(now - 6 * DAY, now, 7));
        assert!(!log_expired(now, now, 7));
    }

    #[test]
    fn 日志_恰好七天边界不删() {
        let now = 1_700_000_000_000;
        // 刚好 7 天整：判定用的是 ">"，所以保留
        assert!(!log_expired(now - 7 * DAY, now, 7));
        assert!(log_expired(now - 7 * DAY - 1, now, 7));
    }

    #[test]
    fn 日志_保留天数为零时只留当天() {
        let now = 1_700_000_000_000;
        assert!(!log_expired(now, now, 0));
        assert!(log_expired(now - 1, now, 0));
    }

    #[test]
    fn 日志_未来时间戳不会被误删() {
        let now = 1_700_000_000_000;
        // 时钟被往回调过的情况：差值是负数，saturating_sub 把 '-' 变成 0
        assert!(!log_expired(now + 5 * DAY, now, 7));
    }
}
