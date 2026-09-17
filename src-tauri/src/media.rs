//! 图片下载、落盘、去重与 LRU 清理（§4.8 关键算法 #6 / #11）。
//!
//! 三条设计约束，改动前先读：
//!  1. **文件名就是 sha256**（`media/<ab>/<sha256>.<ext>`）。同一张图被多条消息引用时只存一份，
//!     而且可以从任意一条消息段的本地路径反推出 sha（`store::message::sha_from_path`），
//!     所以不必往消息段里再塞一份哈希。
//!  2. **渲染绝不请求外链**：webview 只读本地文件（走 `media` 自定义协议）。
//!     上游直链会过期，而且图片可能很大，实时拉取会把滚动帧率打穿。
//!  3. **本库是唯一长期副本**：NapCat 侧的文件标识同样受 LRU 管理，清理 = 永久丢失（§4.4）。
//!     所以清理只在用户显式操作、或总量真的越过上限时才发生。
//!
//! 「纯函数 / 副作用」的分界：路径推导、扩展名猜测、尺寸嗅探、哈希全部是纯函数（带单测）；
//! 网络与磁盘只在 `fetch_and_store` / `cleanup` 里发生。

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::AppHandle;

use crate::appstate::{emit, events, AppState};
use crate::model::{image_state, Peer};
use crate::ob::segments::ImageTask;
use crate::store::{image as image_store, message as message_store};

/// 单张图片的大小上限。超过就不落盘 —— 上游偶尔会把视频封面之类的巨型图塞进图片段，
/// 一张 50 MB 的图能让内存预算直接崩掉（PERF-04）。
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// 下载超时。本机直连（NapCat 与抽屉同机），20 秒已经很宽裕。
pub const DOWNLOAD_TIMEOUT_SECS: u64 = 20;

/// 自定义 URL 协议名。前端 `ipc.imageUrl` 里用 `convertFileSrc(rel_path, 'media')` 指过来，
/// 两边的字符串必须一致。
pub const MEDIA_SCHEME: &str = "media";

/* ------------------------------ 纯函数：路径与命名 ------------------------------ */

/// 相对路径：`<sha 前两位>/<sha>.<ext>`。
///
/// 分两层目录不是为了好看——Windows 的 NTFS 在单目录塞进几万个文件后枚举会明显变慢，
/// 而清理时要扫目录。
pub fn rel_path_of(sha256: &str, ext: &str) -> String {
    let ab = &sha256[..sha256.len().min(2)];
    format!("{}/{}", ab, file_name(sha256, ext))
}

pub fn file_name(sha256: &str, ext: &str) -> String {
    format!("{}.{}", sha256, normalize_ext(ext))
}

fn normalize_ext(ext: &str) -> String {
    let e = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if e.is_empty() {
        "png".to_string()
    } else {
        e
    }
}

/// 绝对落盘路径。
pub fn abs_path(root: &Path, sha256: &str, ext: &str) -> PathBuf {
    let ab = &sha256[..sha256.len().min(2)];
    root.join(ab).join(file_name(sha256, ext))
}

/// 从 URL 的 Content-Type 或路径后缀猜扩展名。
///
/// 优先信 Content-Type：QQ 的图片直链经常是 `...?rkey=xxx` 这种带查询串的形式，
/// 路径后缀会取不到，而且有些 CDN 会把所有图都叫 `.jpg` 却返回 PNG 字节。
pub fn guess_ext(url: &str, content_type: Option<&str>) -> String {
    if let Some(ct) = content_type {
        let ct = ct.split(';').next().unwrap_or(ct).trim().to_ascii_lowercase();
        let mapped = match ct.as_str() {
            "image/png" => Some("png"),
            "image/jpeg" | "image/jpg" => Some("jpg"),
            "image/gif" => Some("gif"),
            "image/webp" => Some("webp"),
            "image/bmp" => Some("bmp"),
            "image/avif" => Some("avif"),
            _ => None,
        };
        if let Some(m) = mapped {
            return m.to_string();
        }
    }

    // 退一步看路径：去掉查询串与 hash，再取最后一个点之后的部分。
    // 后缀限 1~5 个 ASCII 字母数字：真实的图片格式最长也就 4 个字符（jpeg/webp/avif），
    // 更长的基本是 `a.unknownext` 这种把点当普通字符用的路径，当「没后缀」处理更稳。
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let tail = path.rsplit('/').next().unwrap_or(path);
    match tail.rsplit_once('.') {
        Some((_, ext)) if (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) => {
            normalize_ext(ext)
        }
        _ => "png".to_string(),
    }
}

/// 上游的 `sub_type` 归一化。负数与未知值一律当普通图（0）。
///
/// 规格里只需区分「普通图」与「表情/贴纸」（0 / 1 / 2），其余档位渲染上没差别。
pub fn sub_type_of(raw: i32) -> i32 {
    if (0..=2).contains(&raw) {
        raw
    } else {
        0
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// 图片尺寸嗅探。只认几种常见容器的文件头，够用且不引入解码器依赖。
///
/// 拿不到就返回 `None` —— 尺寸只是给前端做占位留白，缺了不影响正确性。
pub fn sniff_size(bytes: &[u8]) -> Option<(i64, i64)> {
    // PNG: 8 字节签名 + 4 字节长度 + "IHDR" + 宽(4) + 高(4)
    if bytes.len() >= 24 && bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some((w as i64, h as i64));
    }

    // GIF: "GIF87a" / "GIF89a" + 宽(2, 小端) + 高(2, 小端)
    if bytes.len() >= 10 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        let w = u16::from_le_bytes([bytes[6], bytes[7]]);
        let h = u16::from_le_bytes([bytes[8], bytes[9]]);
        return Some((w as i64, h as i64));
    }

    // WebP: "RIFF" + 4 字节长度 + "WEBP" + 变体
    if bytes.len() >= 30 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return sniff_webp(bytes);
    }

    // JPEG: 扫 SOF 段
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        return sniff_jpeg(bytes);
    }

    None
}

fn sniff_webp(bytes: &[u8]) -> Option<(i64, i64)> {
    match &bytes[12..16] {
        // VP8X：宽高是 24 位，值减一
        b"VP8X" if bytes.len() >= 30 => {
            let w = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]) & 0x00FF_FFFF;
            let h = u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]) & 0x00FF_FFFF;
            Some(((w + 1) as i64, (h + 1) as i64))
        }
        // VP8L：位打包
        b"VP8L" if bytes.len() >= 25 => {
            let b = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            let w = (b & 0x3FFF) + 1;
            let h = ((b >> 14) & 0x3FFF) + 1;
            Some((w as i64, h as i64))
        }
        // VP8：起始码 9D 01 2A 之后的 14 位宽、14 位高
        b"VP8 " if bytes.len() >= 30 && bytes[23..26] == [0x9D, 0x01, 0x2A] => {
            let w = u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF;
            let h = u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF;
            Some((w as i64, h as i64))
        }
        _ => None,
    }
}

fn sniff_jpeg(bytes: &[u8]) -> Option<(i64, i64)> {
    let mut i = 2usize;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        // SOF0..SOF3 / SOF5..SOF7 / SOF9..SOF11 / SOF13..SOF15
        let is_sof = matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        );
        let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if is_sof {
            if i + 9 >= bytes.len() {
                return None;
            }
            let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]);
            let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]);
            return Some((w as i64, h as i64));
        }
        // 填充字节与无长度字段的标记
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if seg_len < 2 {
            return None;
        }
        i += 2 + seg_len;
    }
    None
}

/* ------------------------------ 落盘 ------------------------------ */

/// 把字节写入媒体目录。已存在则不重写（内容相同，写一遍纯属浪费 I/O）。
fn write_blob(root: &Path, sha256: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf> {
    let dest = abs_path(root, sha256, ext);
    if dest.exists() {
        return Ok(dest);
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("创建图片目录失败: {}", dir.display()))?;
    }
    // 先写临时文件再改名：中途崩掉不会留下一个"看起来完好但截断"的图片
    let tmp = dest.with_extension(format!("{}.part", normalize_ext(ext)));
    std::fs::write(&tmp, bytes).with_context(|| format!("写入图片失败: {}", tmp.display()))?;
    std::fs::rename(&tmp, &dest)
        .with_context(|| format!("落盘图片失败: {}", dest.display()))?;
    Ok(dest)
}

/// 把一段字节按「哈希命名去重」落盘，返回 `(sha256, 绝对路径, 相对路径)`。
///
/// 粘贴图片（FR-26）与下载图片走的是同一条落盘路径 —— 这样"同机发送时只传本地路径"
/// 这个优化对两种情况都成立，缓存统计也只需要一套口径。
pub fn store_bytes(root: &Path, bytes: &[u8], ext_hint: &str) -> Result<(String, String, String)> {
    let sha = sha256_hex(bytes);
    let ext = normalize_ext(ext_hint);
    let path = write_blob(root, &sha, &ext, bytes)?;
    Ok((sha.clone(), path.to_string_lossy().to_string(), rel_path_of(&sha, &ext)))
}

/* ------------------------------ 下载（唯一的网络出口） ------------------------------ */

/// 上游直链下载。同机通信，不需要代理与重定向链，能省就省。
pub async fn download(client: &reqwest::Client, url: &str) -> Result<(Vec<u8>, Option<String>)> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("请求图片失败: {url}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("图片直链返回 {}: {url}", resp.status());
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = resp.bytes().await.context("读取图片字节失败")?.to_vec();
    Ok((bytes, content_type))
}

pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .build()
        .context("创建 HTTP 客户端失败")
}

/// 一条图片任务的完整处理流程：下载 → 去重落盘 → 登记 → 回填消息段 → 通知前端。
///
/// 失败路径也要落状态：`image_state::FAILED`，前端据此显示 `[图片加载失败]`。
/// 只在状态**真的变了**的时候才发 `msg_updated`，避免无谓的前端重绘。
pub async fn fetch_and_store(
    app: &AppHandle,
    state: &Arc<AppState>,
    client: &reqwest::Client,
    message_id: &str,
    task: &ImageTask,
) {
    let outcome = download(client, &task.url).await;

    let (abs, state_code) = match outcome {
        Ok((bytes, content_type)) => {
            if bytes.len() > MAX_IMAGE_BYTES {
                tracing::warn!(
                    message_id,
                    url = %task.url,
                    bytes = bytes.len(),
                    "图片超过大小上限，丢弃"
                );
                (None, image_state::FAILED)
            } else {
                let sha = sha256_hex(&bytes);
                let ext = guess_ext(&task.url, content_type.as_deref());
                let (w, h) = match sniff_size(&bytes) {
                    Some((w, h)) => (Some(w), Some(h)),
                    None => (None, None),
                };
                let root = state.media_root.clone();
                match write_blob(&root, &sha, &ext, &bytes) {
                    Ok(path) => {
                        let rel = rel_path_of(&sha, &ext);
                        let sub = sub_type_of(task.sub_type);
                        let reg = state.db.tx(|c| {
                            image_store::register(
                                c,
                                &sha,
                                &rel,
                                &normalize_ext(&ext),
                                bytes.len() as i64,
                                w,
                                h,
                                sub,
                            )
                        });
                        if let Err(e) = reg {
                            tracing::warn!(error = %e, "图片索引登记失败");
                        }
                        (Some(path.to_string_lossy().to_string()), image_state::READY)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "图片落盘失败");
                        (None, image_state::FAILED)
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, message_id, url = %task.url, "图片下载失败");
            (None, image_state::FAILED)
        }
    };

    let patched = state.db.tx(|c| {
        message_store::patch_image_path(c, message_id, task.index, abs.as_deref(), state_code)
    });
    if let Err(e) = patched {
        tracing::warn!(error = %e, "回填图片路径失败");
        return;
    }

    match state.db.with(|c| message_store::get(c, message_id)) {
        Ok(Some(msg)) => emit(app, events::MSG_UPDATED, msg),
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "读取回填后的消息失败"),
    }
}

/* ------------------------------ 清理 ------------------------------ */

#[derive(Clone, Debug, Default, Serialize)]
pub struct CleanupReport {
    pub removed: i64,
    pub freed_bytes: i64,
}

impl CleanupReport {
    pub fn is_empty(&self) -> bool {
        self.removed == 0
    }
}

/// 真正删文件。失败只记日志 —— 索引行已经删了，为了一个残留文件让整次清理失败不值得。
fn remove_file(root: &Path, sha: &str) {
    // 扩展名不写死在协议里：扫 `<ab>` 目录下以 sha 开头的文件
    let ab = root.join(&sha[..sha.len().min(2)]);
    let Ok(entries) = std::fs::read_dir(&ab) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(sha) {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                tracing::warn!(error = %e, path = %entry.path().display(), "删除图片文件失败");
            }
        }
    }
}

/// 按 LRU 清理一批 sha：先删文件，再删索引行并把引用它们的消息段置为「已清理」。
pub fn purge_shas(state: &Arc<AppState>, shas: &[String]) -> CleanupReport {
    if shas.is_empty() {
        return CleanupReport::default();
    }
    let freed = state
        .db
        .tx(|c| image_store::purge(c, shas))
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "清理图片索引失败");
            0
        });
    for sha in shas {
        remove_file(&state.media_root, sha);
    }
    CleanupReport { removed: shas.len() as i64, freed_bytes: freed }
}

/// 按保留天数 + 容量上限做一次 LRU（§4.8 #11）。启动时与后台定时任务里调用。
pub fn auto_cleanup(state: &Arc<AppState>, keep_days: i64, limit_bytes: i64) -> CleanupReport {
    let candidates = state
        .db
        .with(|c| image_store::lru_candidates(c, keep_days, limit_bytes))
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "计算待清理图片失败");
            Vec::new()
        });
    purge_shas(state, &candidates)
}

/// 「清空缓存」的三种口径（FR-42）。
pub fn clear(
    state: &Arc<AppState>,
    scope: &str,
    peer: Option<Peer>,
    days: i64,
    keep_days: i64,
    limit_bytes: i64,
) -> CleanupReport {
    match scope {
        "peer" => match peer {
            Some(p) => {
                let shas = state
                    .db
                    .with(|c| image_store::shas_of_peer(c, p))
                    .unwrap_or_default();
                purge_shas(state, &shas)
            }
            None => CleanupReport::default(),
        },
        "age" => {
            let oldest = if days > 0 { days } else { keep_days };
            let candidates = state
                .db
                .with(|c| image_store::lru_candidates(c, oldest, 0))
                .unwrap_or_default();
            purge_shas(state, &candidates)
        }
        // "all"：绕过保留天数，直接按总量上限为 0 触发全量 LRU
        _ => {
            let _ = limit_bytes;
            let candidates = state
                .db
                .with(|c| image_store::lru_candidates(c, 0, 0))
                .unwrap_or_default();
            // lru_candidates 在 keep_days=0 且 limit_bytes=0 时不会给出候选，
            // 所以这里退回「列全部索引行」
            let candidates = if candidates.is_empty() {
                state
                    .db
                    .with(|c| {
                        let mut stmt = c.prepare("SELECT sha256 FROM image_cache")?;
                        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
                    })
                    .unwrap_or_default()
            } else {
                candidates
            };
            purge_shas(state, &candidates)
        }
    }
}

/// 触发一次「渲染时刷新 LRU」（前端图片真正显示出来时调用）。
pub fn touch(state: &Arc<AppState>, sha256: &str) {
    if let Err(e) = state.db.tx(|c| image_store::touch(c, sha256)) {
        tracing::debug!(error = %e, "刷新图片访问时间失败");
    }
}

/// 给 `tauri::Builder::register_uri_scheme_protocol` 用的请求解析：
/// `http://media.localhost/<ab>/<sha>.<ext>` → `media_root/<ab>/<sha>.<ext>`。
///
/// 返回 `None` 表示这个路径不该被服务（含 `..`、绝对路径、空路径）。
/// 这是唯一的路径入口，**必须**在这里拦住目录穿越。
pub fn resolve_request(root: &Path, raw_path: &str) -> Option<PathBuf> {
    let rel = raw_path.trim_start_matches('/');
    if rel.is_empty() {
        return None;
    }
    // percent-decode 一个最小子集即可：文件名只会是十六进制与点
    let decoded = percent_decode(rel);
    let p = Path::new(&decoded);
    if p.is_absolute() {
        return None;
    }
    for part in p.components() {
        match part {
            std::path::Component::Normal(_) => {}
            _ => return None,
        }
    }
    let full = root.join(p);
    // 双保险：规范化之后必须仍在 root 之下
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if let Ok(canon) = full.canonicalize() {
        if !canon.starts_with(&canon_root) {
            return None;
        }
        return Some(canon);
    }
    // 文件还不存在时 canonicalize 会失败；此时上面的组件检查已经足够
    Some(full)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 扩展名 → MIME。认不出的按 `image/png` 处理（WebView2 对未知类型会拒绝渲染，
/// 而我们这个协议只会被 `<img>` 使用）。
pub fn mime_of_ext(ext: &str) -> &'static str {
    match normalize_ext(ext).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// 一次自定义协议请求的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served {
    pub status: u16,
    pub mime: &'static str,
    pub body: Vec<u8>,
}

impl Served {
    fn not_found() -> Self {
        Self { status: 404, mime: "text/plain; charset=utf-8", body: Vec::new() }
    }
    fn bad_request() -> Self {
        Self { status: 400, mime: "text/plain; charset=utf-8", body: Vec::new() }
    }
    pub fn is_ok(&self) -> bool {
        self.status == 200
    }
}

/// 处理 `media://` 请求：把 `/<ab>/<sha>.<ext>` 读成字节返回。
///
/// 读文件是唯一的副作用；路径安全全部交给 `resolve_request`，
/// 所以这个函数本身可以放心地在协议回调里同步执行（图片都在本地，读盘是毫秒级）。
pub fn serve(root: &Path, request_path: &str) -> Served {
    let Some(full) = resolve_request(root, request_path) else {
        tracing::debug!(request_path, "自定义协议拒绝了越界路径");
        return Served::bad_request();
    };
    match std::fs::read(&full) {
        Ok(body) => {
            let ext = full.extension().and_then(|e| e.to_str()).unwrap_or("");
            Served { status: 200, mime: mime_of_ext(ext), body }
        }
        Err(e) => {
            // 缓存被清理后图片段会变成「已清理」，但前端可能还拿着旧地址请求一次
            tracing::debug!(error = %e, path = %full.display(), "读取图片失败");
            Served::not_found()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn 相对路径按前两位分目录() {
        assert_eq!(
            rel_path_of(SHA, "png"),
            "01/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.png"
        );
    }

    #[test]
    fn 扩展名被归一化() {
        assert_eq!(file_name(SHA, ".PNG"), format!("{SHA}.png"));
        assert_eq!(file_name(SHA, ""), format!("{SHA}.png"));
        assert_eq!(file_name(SHA, "  jpg "), format!("{SHA}.jpg"));
    }

    #[test]
    fn 绝对路径挂在媒体目录下() {
        let root = Path::new("C:/d/qq-drawer/media");
        let p = abs_path(root, SHA, "png");
        // 用 Path 比，别拼字符串断言：Windows 上 join 出来是 `01\<sha>.png`，
        // 断言 contains("/01/") 会假失败（这正是它一直红着的原因）。
        let rel = p.strip_prefix(root).expect("必须落在媒体目录下");
        assert_eq!(rel, Path::new("01").join(format!("{SHA}.png")));
    }

    #[test]
    fn 扩展名优先取_content_type() {
        assert_eq!(guess_ext("http://x/a.jpg?rkey=1", Some("image/png")), "png");
        assert_eq!(guess_ext("http://x/a", Some("image/jpeg; charset=binary")), "jpg");
        assert_eq!(guess_ext("http://x/a.webp", None), "webp");
        assert_eq!(guess_ext("http://x/a.png?v=2#f", None), "png");
        assert_eq!(guess_ext("http://x/noext", None), "png");
        // 后缀超过 5 个字符就不像图片格式了，按「没后缀」处理，退回默认 png
        assert_eq!(guess_ext("http://x/a.unknownext", None), "png");
    }

    #[test]
    fn 子类型只认普通图与表情() {
        assert_eq!(sub_type_of(0), 0);
        assert_eq!(sub_type_of(1), 1);
        assert_eq!(sub_type_of(2), 2);
        assert_eq!(sub_type_of(7), 0, "贴图等档位统一当普通图");
        assert_eq!(sub_type_of(-1), 0);
    }

    #[test]
    fn 哈希稳定() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(sha256_hex(b"abc").len(), 64);
    }

    #[test]
    fn 嗅探_png() {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        b.extend_from_slice(&[0, 0, 0, 13]);
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&800u32.to_be_bytes());
        b.extend_from_slice(&600u32.to_be_bytes());
        assert_eq!(sniff_size(&b), Some((800, 600)));
    }

    #[test]
    fn 嗅探_gif() {
        let mut b = b"GIF89a".to_vec();
        b.extend_from_slice(&320u16.to_le_bytes());
        b.extend_from_slice(&240u16.to_le_bytes());
        assert_eq!(sniff_size(&b), Some((320, 240)));
    }

    #[test]
    fn 嗅探_jpeg() {
        // SOI + SOF0 段
        let mut b: Vec<u8> = vec![0xFF, 0xD8];
        b.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]); // APP0，长度 4
        b.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]); // SOF0，长度 17，精度 8
        b.extend_from_slice(&1080u16.to_be_bytes()); // 高
        b.extend_from_slice(&1920u16.to_be_bytes()); // 宽
        b.extend_from_slice(&[0x03, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(sniff_size(&b), Some((1920, 1080)));
    }

    #[test]
    fn 嗅探_认不出就返回空() {
        assert_eq!(sniff_size(b"not an image at all"), None);
        assert_eq!(sniff_size(&[]), None);
    }

    #[test]
    fn 路径解析_拦截目录穿越() {
        let root = Path::new("C:/d/qq-drawer/media");
        assert!(resolve_request(root, "/01/abc.png").is_some());
        assert!(resolve_request(root, "/../secret.txt").is_none());
        assert!(resolve_request(root, "/01/../../x.png").is_none());
        assert!(resolve_request(root, "/C:/Windows/win.ini").is_none());
        assert!(resolve_request(root, "/").is_none());
        assert!(resolve_request(root, "").is_none());
    }

    #[test]
    fn 百分号解码() {
        assert_eq!(percent_decode("a%20b.png"), "a b.png");
        assert_eq!(percent_decode("plain.png"), "plain.png");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn 空清理报告不触发副作用() {
        assert!(CleanupReport::default().is_empty());
        assert!(!CleanupReport { removed: 1, freed_bytes: 0 }.is_empty());
    }

    #[test]
    fn mime_映射() {
        assert_eq!(mime_of_ext("png"), "image/png");
        assert_eq!(mime_of_ext(".JPG"), "image/jpeg");
        assert_eq!(mime_of_ext("jpeg"), "image/jpeg");
        assert_eq!(mime_of_ext("gif"), "image/gif");
        assert_eq!(mime_of_ext("webp"), "image/webp");
        assert_eq!(mime_of_ext("不认识的"), "application/octet-stream");
    }

    #[test]
    fn 协议_越界路径返回_400() {
        let root = Path::new("C:/d/qq-drawer/media");
        assert_eq!(serve(root, "/../secret").status, 400);
        assert_eq!(serve(root, "/").status, 400);
        assert_eq!(serve(root, "").status, 400);
    }

    #[test]
    fn 协议_文件不存在返回_404() {
        let root = std::env::temp_dir().join("qq-drawer-不存在的目录-xyz");
        let s = serve(&root, "/ab/deadbeef.png");
        assert_eq!(s.status, 404);
        assert!(!s.is_ok());
    }

    #[test]
    fn 协议_能读出刚写下的图片() {
        let root = std::env::temp_dir().join(format!("qq-drawer-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bytes = b"\x89PNG\r\n\x1a\n fake png body";
        let (sha, _abs, rel) = store_bytes(&root, bytes, "png").unwrap();

        let s = serve(&root, &format!("/{rel}"));
        assert!(s.is_ok(), "刚落盘的图片必须能读出来");
        assert_eq!(s.mime, "image/png");
        assert_eq!(s.body, bytes);

        // 同一份字节再落一次不会产生新文件（内容寻址去重）
        let (sha2, _, rel2) = store_bytes(&root, bytes, "png").unwrap();
        assert_eq!(sha, sha2);
        assert_eq!(rel, rel2);

        let _ = std::fs::remove_dir_all(&root);
    }
}
