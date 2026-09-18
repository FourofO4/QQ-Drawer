//! 消息段解析与序列化（§4.7）。
//!
//! 两个方向都在这里：
//!  · **接收**：上游消息段数组 → `Vec<Seg>`（规范模型）；同时挑出需要下载的图片；
//!  · **发送**：输入层令牌 → OneBot 消息段数组。
//!
//! 解析原则是"宁可降级也不失败"：认不出的段一律变成 `[暂不支持的消息类型]` 占位，
//! 而原始 JSON 已经整条入库，日后想补渲染还来得及（§4.7）。

use serde_json::{json, Value};

use crate::model::{image_state, OutgoingPart, Seg};

/// 一条待下载的图片
#[derive(Clone, Debug, PartialEq)]
pub struct ImageTask {
    /// 在 `segments` 里的下标，下载完成后按这个下标回填路径
    pub index: usize,
    pub url: String,
    pub sub_type: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub segments: Vec<Seg>,
    /// 是否 @ 了我
    pub is_at_me: bool,
    pub images: Vec<ImageTask>,
}

impl Parsed {
    pub fn has_image(&self) -> bool {
        !self.images.is_empty()
    }

    /// 纯文本合并（入库的 `message.text`）
    pub fn plain_text(&self) -> String {
        let joined: String = self.segments.iter().map(|s| s.as_plain()).collect();
        crate::store::message::flatten(&joined)
    }

    /// 需要向群成员表查名字的 qq 号（`at` 段里没带名字的那些）
    pub fn unknown_at_names(&self) -> Vec<i64> {
        self.segments
            .iter()
            .filter_map(|s| match s {
                Seg::At { qq, name: None, is_self: false } => Some(*qq),
                _ => None,
            })
            .collect()
    }

    /// 把查到的名字回填进 `at` 段
    pub fn fill_at_name(&mut self, qq: i64, name: &str) {
        for s in self.segments.iter_mut() {
            if let Seg::At { qq: q, name: n, .. } = s {
                if *q == qq && n.is_none() {
                    *n = Some(name.to_string());
                }
            }
        }
    }
}

/// 图片段一律按缩略占位处理：真正的像素要等下载完并落盘。
fn image_from(data: &Value) -> (Seg, Option<String>) {
    let sub_type = data
        .get("sub_type")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i32>().ok())
        .or_else(|| data.get("sub_type").and_then(|v| v.as_i64()).map(|v| v as i32))
        .unwrap_or(0);

    // 上游给的是 http(s) 直链；`file` 只是文件名，不能直接下载
    let url = data
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| s.starts_with("http"))
        .map(|s| s.to_string());

    let state = if url.is_some() {
        image_state::DOWNLOADING
    } else {
        image_state::FAILED
    };

    (Seg::Image { path: None, sub_type, state }, url)
}

/// 拆开 QQ 机器人 `markdown` 段的 `content`，得到「图片直链 / @ 的名字 / 剩下的可见文字」。
///
/// 这个段只有一个 `content` 字段，内容是普通 markdown 的混排，实测形态：
///
/// ```text
/// [@FourofO4](mqqapi://markdown/mention?at_type=1&at_tinyid=734918143)
/// ![img #1116px #3548px](https://qqbot.ugcimg.cn/1905536814/….jpg)
/// ```
///
/// 图片拎出来走正常的下载落盘（`Seg::Image`），链接只保留标签文字 ——
/// 机器人几乎只用 markdown 干这两件事。**认不出的部分原样留在文字里**，
/// 宁可显示得糙一点，也不要凭空吞掉内容。
fn markdown_parts(content: &str) -> (Vec<String>, Vec<String>, String) {
    let mut images = Vec::new();
    let mut mentions = Vec::new();
    let mut text = String::new();
    let mut i = 0usize;

    while i < content.len() {
        let Some(rel) = content[i..].find('[') else {
            text.push_str(&content[i..]);
            break;
        };
        let open = i + rel;
        // `![alt](url)` 才是图片；`[label](url)` 是链接
        let is_image = open > i && content.as_bytes()[open - 1] == b'!';

        // 找 `]` 之后紧跟的 `(...)`。少任何一半都当普通字符处理，不做深究
        let rest = &content[open + 1..];
        let parsed = rest.find(']').and_then(|c| {
            let after = &rest[c + 1..];
            after.strip_prefix('(')?.find(')').map(|e| (c, e))
        });
        let Some((close, paren)) = parsed else {
            // 多带一个字符，免得把位于 `[` 前面的 `!` 丢掉
            text.push_str(&content[i..open + 1]);
            i = open + 1;
            continue;
        };

        // 标签之前的字面量；图片语法那个 `!` 不算内容
        text.push_str(&content[i..if is_image { open - 1 } else { open }]);
        let label = &rest[..close];
        let url = &rest[close + 2..close + 2 + paren];

        if is_image {
            if url.starts_with("http") {
                images.push(url.to_string());
            }
        } else if url.starts_with("mqqapi://") {
            mentions.push(label.to_string());
        } else {
            text.push_str(label);
        }
        i = open + close + paren + 4;
    }

    (images, mentions, text)
}

/// 解析上游消息段数组。
///
/// `self_id` 用来判定 @我；@ 的具体昵称留给调用方从成员表补齐
/// （解析阶段不碰数据库，这样这段逻辑可以纯函数测试）。
pub fn parse(raw: &[Value], self_id: i64) -> Parsed {
    let mut out = Parsed::default();

    // 这条消息别处有没有等价的段。NapCat 会把纯文本的 markdown **再给一份 `text`**，
    // 机器人 @ 人时也常常 markdown 链接和 `at` 段同时出现；不先看一眼就会渲染成两份。
    let has_text = raw.iter().any(|it| {
        it.get("type").and_then(|v| v.as_str()) == Some("text")
            && it
                .get("data")
                .and_then(|d| d.get("text"))
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
    });
    let has_at = raw.iter().any(|it| it.get("type").and_then(|v| v.as_str()) == Some("at"));

    for item in raw {
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let data = item.get("data").cloned().unwrap_or(Value::Null);

        match kind {
            "text" => {
                let text = data
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                if !text.is_empty() {
                    out.segments.push(Seg::text(text));
                }
            }
            "image" => {
                let (seg, url) = image_from(&data);
                if let Some(u) = url {
                    let sub_type = match &seg {
                        Seg::Image { sub_type, .. } => *sub_type,
                        _ => 0,
                    };
                    out.images.push(ImageTask { index: out.segments.len(), url: u, sub_type });
                }
                out.segments.push(seg);
            }
            "at" => {
                let qq = data
                    .get("qq")
                    .and_then(|v| {
                        v.as_i64()
                            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                    })
                    .unwrap_or(0);
                let is_self = qq == self_id;
                if is_self {
                    out.is_at_me = true;
                }
                let name = data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                out.segments.push(Seg::At { qq, name, is_self });
            }
            "reply" => {
                let id = data
                    .get("id")
                    .and_then(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .or_else(|| v.as_i64().map(|n| n.to_string()))
                    })
                    .unwrap_or_default();
                if !id.is_empty() {
                    out.segments.push(Seg::Reply { id });
                }
            }
            "markdown" => {
                let content = data.get("content").and_then(|v| v.as_str()).unwrap_or_default();
                let (urls, mentions, text) = markdown_parts(content);
                // 一个字都没拆出来（空 content、或只有一张没带直链的图）才算"看不懂"
                let nothing = urls.is_empty() && mentions.is_empty() && text.trim().is_empty();

                for u in urls {
                    let (seg, url) = image_from(&json!({ "url": u }));
                    if let Some(u) = url {
                        let sub_type = match &seg {
                            Seg::Image { sub_type, .. } => *sub_type,
                            _ => 0,
                        };
                        out.images.push(ImageTask { index: out.segments.len(), url: u, sub_type });
                    }
                    out.segments.push(seg);
                }
                if !has_at {
                    for m in mentions {
                        out.segments.push(Seg::text(m));
                    }
                }
                // 前后空白不留（`flatten` 本来也会收敛），但紧跟在 @ 后面的那个空格要保住：
                // `[@某人](…) 在吗` 这类把 @ 和正文写在一行里的内容，丢了空格会粘成 `@某人在吗`
                let tail = text.trim();
                if !tail.is_empty() && !has_text {
                    let lead = if text.starts_with(char::is_whitespace)
                        && out.segments.last().is_some_and(|s| matches!(s, Seg::Text { .. }))
                    {
                        " "
                    } else {
                        ""
                    };
                    out.segments.push(Seg::text(format!("{lead}{tail}")));
                }
                // 内容认不出来 → 还是退回卡片占位
                if nothing {
                    out.segments.push(Seg::placeholder("card", "[卡片消息]"));
                }
            }
            "face" | "mface" => out.segments.push(Seg::placeholder("face", "[表情]")),
            "record" => out.segments.push(Seg::placeholder("record", "[语音]")),
            "video" => out.segments.push(Seg::placeholder("video", "[视频]")),
            "file" => out.segments.push(Seg::placeholder("file", "[文件]")),
            "json" | "xml" => out.segments.push(Seg::placeholder("card", "[卡片消息]")),
            "forward" | "node" => out.segments.push(Seg::placeholder("forward", "[合并转发]")),
            // 戳一戳等一律忽略，不进消息体（§4.5）
            "poke" | "shake" => {}
            _ => out.segments.push(Seg::placeholder("unknown", "[暂不支持的消息类型]")),
        }
    }

    if out.segments.is_empty() {
        out.segments.push(Seg::placeholder("unknown", "[暂不支持的消息类型]"));
    }
    out
}

/// 输入层令牌 → OneBot 消息段数组（§4.7 发送）。
///
/// 关键取舍：图片用**本地绝对路径**而不是 base64。
/// 抽屉与 NapCat 同机，WebSocket 只传几十字节，几 MB 的截图不会撑爆连接，
/// 也避免 base64 的内存尖峰（§4.6）。
pub fn build_outgoing(parts: &[OutgoingPart]) -> Value {
    let mut arr = Vec::with_capacity(parts.len());
    for p in parts {
        match p {
            OutgoingPart::Reply { id } => {
                arr.push(json!({ "type": "reply", "data": { "id": id } }));
            }
            OutgoingPart::Text { text } => {
                if !text.is_empty() {
                    arr.push(json!({ "type": "text", "data": { "text": text } }));
                }
            }
            OutgoingPart::At { qq, .. } => {
                // 不做 at all（§4.7：群聊专用，不支持全体）
                arr.push(json!({ "type": "at", "data": { "qq": qq.to_string() } }));
            }
            OutgoingPart::Image { path, .. } => {
                arr.push(json!({ "type": "image", "data": { "file": path } }));
            }
        }
    }
    Value::Array(arr)
}

/// 发送前的最后一道校验。前端已经拦过一次，这里是防"绕过前端"的兜底。
pub fn validate_outgoing(parts: &[OutgoingPart], is_group: bool) -> Result<(), String> {
    let has_content = parts.iter().any(|p| match p {
        OutgoingPart::Text { text } => !text.trim().is_empty(),
        OutgoingPart::At { .. } | OutgoingPart::Image { .. } => true,
        OutgoingPart::Reply { .. } => false,
    });
    if !has_content {
        return Err("没有可发送的内容".into());
    }
    if !is_group && parts.iter().any(|p| matches!(p, OutgoingPart::At { .. })) {
        return Err("私聊里不能 @ 人".into());
    }
    Ok(())
}

/// 从 `get_recent_contact` 的返回里抠出会话种子（FR-14）。
///
/// ⚠️ 这个接口在不同版本间有**两种返回形态**（§4.6）：
///   A: `peerUin / peerName / msgTime / lastestMsg`
///   B: `user_id / group_id / time / message_type / raw_message / sender`
/// 两种都认：优先用明确的 `group_id` / `user_id`；只有形态 A 的 `peerUin` 时，
/// 再看 `chatType` / `message_type` 判断会话类型，判断不了就按私聊处理。
/// M1 实测后按本机实际形态收敛。
pub fn recent_contact_seed(items: &[Value]) -> Vec<(crate::model::Peer, String, Option<i64>)> {
    let mut out = Vec::new();
    for it in items {
        let explicit_group = super::model::as_i64(it, "group_id").filter(|v| *v > 0);
        let explicit_user = super::model::as_i64(it, "user_id").filter(|v| *v > 0);
        let peer_uin = super::model::as_i64(it, "peerUin").filter(|v| *v > 0);

        let peer = if let Some(gid) = explicit_group {
            crate::model::Peer::group(gid)
        } else if let Some(uid) = explicit_user {
            crate::model::Peer::private(uid)
        } else if let Some(uin) = peer_uin {
            let is_group = super::model::as_i64(it, "chatType").map(|v| v == 2).unwrap_or(false)
                || super::model::as_str(it, "message_type").as_deref() == Some("group")
                || super::model::as_str(it, "type").as_deref() == Some("group");
            if is_group {
                crate::model::Peer::group(uin)
            } else {
                crate::model::Peer::private(uin)
            }
        } else {
            continue;
        };

        let name = super::model::as_str(it, "peerName")
            .or_else(|| super::model::as_str(it, "group_name"))
            .or_else(|| super::model::as_str(it, "nickname"))
            .or_else(|| super::model::deep_str(it, &["sender", "nickname"]))
            .or_else(|| super::model::as_str(it, "card"))
            .unwrap_or_default();

        let ts = super::model::as_i64(it, "msgTime")
            .or_else(|| super::model::as_i64(it, "time"))
            .map(super::model::normalize_ts);

        out.push((peer, name, ts));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(v: Value) -> Vec<Value> {
        v.as_array().cloned().unwrap_or_default()
    }

    #[test]
    fn 解析_文本与艾特我() {
        let p = parse(
            &raw(json!([
                { "type": "at", "data": { "qq": "10001" } },
                { "type": "text", "data": { "text": " 字段名定了没？" } }
            ])),
            10001,
        );
        assert!(p.is_at_me);
        assert_eq!(p.segments.len(), 2);
        // 艾特自己走的是固定文案 `@你`（`model::Seg::as_plain`，规格书 §4.7 的界面示意
        // 画的也是「@你 那个接口的字段名定了没？」）。不是 `@10001`：给用户看 QQ 号
        // 没意义，而 `@昵称` 又要等成员表回填，所以自己这一档直接写死。
        assert_eq!(p.plain_text(), "@你 字段名定了没？");
        // 自己这一档不需要查名字：文案是写死的 `@你`，所以不计入待查名单
        assert!(p.unknown_at_names().is_empty());
    }

    #[test]
    fn 解析_艾特别人不算艾特我() {
        let p = parse(&raw(json!([{ "type": "at", "data": { "qq": "30011" } }])), 10001);
        assert!(!p.is_at_me);
        // 别人的 at 且上游没给名字 → 要进待查名单，等群成员表回填（见 `解析_名字能被回填`）
        assert_eq!(p.unknown_at_names(), vec![30011]);
        assert_eq!(p.plain_text(), "@30011");
    }

    #[test]
    fn 解析_艾特带昵称时不再查成员表() {
        let p = parse(
            &raw(json!([{ "type": "at", "data": { "qq": "30011", "name": "李工" } }])),
            10001,
        );
        assert!(p.unknown_at_names().is_empty());
        match &p.segments[0] {
            Seg::At { name, .. } => assert_eq!(name.as_deref(), Some("李工")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 解析_名字能被回填() {
        let mut p = parse(&raw(json!([{ "type": "at", "data": { "qq": "30011" } }])), 10001);
        p.fill_at_name(30011, "李工");
        assert_eq!(p.plain_text(), "@李工");
    }

    #[test]
    fn 解析_图片入队下载且标记为下载中() {
        let p = parse(
            &raw(json!([
                { "type": "text", "data": { "text": "看图" } },
                { "type": "image", "data": { "url": "http://127.0.0.1:3000/a.png", "sub_type": "1" } }
            ])),
            10001,
        );
        assert_eq!(p.images.len(), 1);
        assert_eq!(p.images[0].index, 1);
        assert_eq!(p.images[0].sub_type, 1);
        assert!(p.has_image());
        match &p.segments[1] {
            Seg::Image { path, state, sub_type } => {
                assert!(path.is_none());
                assert_eq!(*state, image_state::DOWNLOADING);
                assert_eq!(*sub_type, 1);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 解析_图片没有直链时标记失败() {
        let p = parse(&raw(json!([{ "type": "image", "data": { "file": "abc.jpg" } }])), 1);
        assert!(p.images.is_empty());
        match &p.segments[0] {
            Seg::Image { state, .. } => assert_eq!(*state, image_state::FAILED),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 解析_引用段() {
        let p = parse(&raw(json!([{ "type": "reply", "data": { "id": 12345 } }])), 1);
        assert_eq!(p.segments, vec![Seg::Reply { id: "12345".into() }]);
    }

    #[test]
    fn 解析_各类占位与文案一致() {
        let p = parse(
            &raw(json!([
                { "type": "face", "data": { "id": 1 } },
                { "type": "mface", "data": {} },
                { "type": "record", "data": {} },
                { "type": "video", "data": {} },
                { "type": "file", "data": {} },
                { "type": "json", "data": {} },
                { "type": "xml", "data": {} },
                { "type": "forward", "data": {} },
                { "type": "未知类型", "data": {} }
            ])),
            1,
        );
        assert_eq!(
            p.plain_text(),
            "[表情][表情][语音][视频][文件][卡片消息][卡片消息][合并转发][暂不支持的消息类型]"
        );
    }

    #[test]
    fn 解析_戳一戳被忽略() {
        let p = parse(
            &raw(json!([
                { "type": "poke", "data": { "qq": "1" } },
                { "type": "text", "data": { "text": "在" } }
            ])),
            1,
        );
        assert_eq!(p.plain_text(), "在");
    }

    /* ---- markdown（QQ 机器人的富文本，实测是群里最容易被漏掉的一类） ---- */

    #[test]
    fn markdown拆解_图片艾特文字混排() {
        let (imgs, ats, text) = markdown_parts(
            "[@小明](mqqapi://markdown/mention?at_type=1&at_tinyid=1)\n\
             ![img #1116px #3548px](https://qqbot.ugcimg.cn/a/b.jpg)\n收工",
        );
        assert_eq!(imgs, vec!["https://qqbot.ugcimg.cn/a/b.jpg"]);
        assert_eq!(ats, vec!["@小明"]);
        assert_eq!(text.trim(), "收工", "夹在语法之间的字面量不能丢");
    }

    #[test]
    fn markdown拆解_语法不完整时原样保留() {
        let (imgs, ats, text) = markdown_parts("看图 ![img](https://a/b");
        assert!(imgs.is_empty());
        assert!(ats.is_empty());
        assert_eq!(text, "看图 ![img](https://a/b", "少了右括号就整段当文字，不吞字");
    }

    #[test]
    fn 解析_markdown里的图片入队下载() {
        let p = parse(
            &raw(json!([{
                "type": "markdown",
                "data": { "content": "![img #1116px #3548px](https://qqbot.ugcimg.cn/a/b.jpg)" }
            }])),
            1,
        );
        assert_eq!(p.images.len(), 1);
        assert_eq!(p.images[0].url, "https://qqbot.ugcimg.cn/a/b.jpg");
        assert_eq!(p.images[0].index, 0);
        assert!(p.has_image(), "markdown 里的图也要算进 has_image");
        assert_eq!(p.plain_text(), "[图片]");
    }

    #[test]
    fn 解析_markdown纯文本不重复成两句() {
        // NapCat 对纯文本 markdown 会额外再给一份 `text` 段，直接照抄会渲染成两句
        let p = parse(
            &raw(json!([
                { "type": "markdown", "data": { "content": "确认好了，回网页那边。" } },
                { "type": "text", "data": { "text": "确认好了，回网页那边。" } }
            ])),
            1,
        );
        assert_eq!(p.segments, vec![Seg::text("确认好了，回网页那边。")]);
    }

    #[test]
    fn 解析_markdown纯文本没有text段时降级为文本() {
        let p = parse(&raw(json!([{ "type": "markdown", "data": { "content": "任务已完成" } }])), 1);
        assert_eq!(p.segments, vec![Seg::text("任务已完成")]);
    }

    #[test]
    fn 解析_markdown的艾特在已有at段时不重复() {
        let p = parse(
            &raw(json!([
                {
                    "type": "markdown",
                    "data": {
                        "content": "[@FourofO4](mqqapi://markdown/mention?at_type=1&at_tinyid=734918143)\n![img](https://qqbot.ugcimg.cn/a.jpg)"
                    }
                },
                { "type": "at", "data": { "qq": "734918143" } }
            ])),
            3431439965,
        );
        assert_eq!(p.segments.len(), 2, "只留图片段和 at 段：{:?}", p.segments);
        assert!(matches!(p.segments[0], Seg::Image { .. }));
        assert!(matches!(p.segments[1], Seg::At { .. }));
    }

    #[test]
    fn 解析_markdown的艾特没有at段时变成文本() {
        let p = parse(
            &raw(json!([{
                "type": "markdown",
                "data": { "content": "[@小明](mqqapi://markdown/mention?at_type=1&at_tinyid=1) 在吗" }
            }])),
            1,
        );
        assert_eq!(p.plain_text(), "@小明 在吗", "@ 和正文之间的空格要留住");
    }

    #[test]
    fn 解析_markdown普通链接只留标签() {
        let p = parse(
            &raw(json!([{ "type": "markdown", "data": { "content": "看[这里](https://example.com/x)吧" } }])),
            1,
        );
        assert_eq!(p.plain_text(), "看这里吧");
    }

    #[test]
    fn 解析_markdown认不出来时退回卡片占位() {
        // 空 content、以及字段整个缺失：都要有兜底文案，不能渲染成空白
        assert_eq!(
            parse(&raw(json!([{ "type": "markdown", "data": { "content": "" } }])), 1).plain_text(),
            "[卡片消息]"
        );
        assert_eq!(
            parse(&raw(json!([{ "type": "markdown", "data": {} }])), 1).plain_text(),
            "[卡片消息]"
        );
    }

    #[test]
    fn 解析_markdown真实样例回放() {
        // 下面四条是从本机 NapCat `get_msg` 抓回来的**原始** message 数组，一字未改。
        // 都是群里那个 QQ 机器人（小饭卡）发的，修复前一律渲染成 `[暂不支持的消息类型]`。
        let cases = [
            // ① 纯图片
            (
                json!([{ "type": "markdown", "data": { "content": "![img #1116px #3548px](https://qqbot.ugcimg.cn/1905536814/470ec7b1c875830136c49c7c45dd33321b1a28e6/fb0eb8d520c224c9a1b14a0bcdea067a)" } }]),
                "[图片]",
            ),
            // ② 纯文本，NapCat 另外补了一份 `text`，不能渲染成两句
            (
                json!([
                    { "type": "markdown", "data": { "content": "确认好了，回网页那边，它会自己开始传。" } },
                    { "type": "text", "data": { "text": "确认好了，回网页那边，它会自己开始传。" } }
                ]),
                "确认好了，回网页那边，它会自己开始传。",
            ),
            // ③ @ 在 markdown 里、同时另有 `at` 段
            (
                json!([
                    { "type": "markdown", "data": { "content": "[@FourofO4](mqqapi://markdown/mention?at_type=1&at_tinyid=734918143)\n![img #1116px #3548px](https://qqbot.ugcimg.cn/1905536814/7cbbe644373c2fc145f4b2b0f3494ee4550da26d/809642dfc41a98e508f7f7b93a8d4605)" } },
                    { "type": "at", "data": { "qq": "734918143" } }
                ]),
                // 段序就是上游给的顺序：markdown 在前、`at` 在后
                "[图片]@734918143",
            ),
            // ④ 纯图片，另一条
            (
                json!([{ "type": "markdown", "data": { "content": "![img #1800px #4894px](https://qqbot.ugcimg.cn/1905536814/de2fce6321521440006dbc8444d902628f5f0398/eb546f8280bd945b7b06786cd3c106b0)" } }]),
                "[图片]",
            ),
        ];

        for (i, (payload, want)) in cases.iter().enumerate() {
            let p = parse(&raw(payload.clone()), 3431439965);
            assert_eq!(p.plain_text(), *want, "样例 {} 回放结果不对", i + 1);
            assert!(!p.segments.is_empty());
        }

        // ①④ 各有一张图，③ 也有一张 → 三张图都要真的进下载队列
        let imgs: Vec<String> = cases
            .iter()
            .flat_map(|(v, _)| parse(&raw(v.clone()), 3431439965).images)
            .map(|t| t.url)
            .collect();
        assert_eq!(imgs.len(), 3, "三条图片位置都对不上：{imgs:?}");
        assert!(imgs.iter().all(|u| u.starts_with("https://qqbot.ugcimg.cn/")));
    }

    #[test]
    fn 解析_空段列表降级成占位() {
        let p = parse(&[], 1);
        assert_eq!(p.plain_text(), "[暂不支持的消息类型]");
    }

    #[test]
    fn 序列化_文本艾特图片() {
        let parts = vec![
            OutgoingPart::Text { text: "来了 ".into() },
            OutgoingPart::At { qq: 30011, name: "李工".into() },
            OutgoingPart::Image { sha256: "x".into(), path: "C:/m/a.png".into() },
        ];
        let v = build_outgoing(&parts);
        assert_eq!(v[0]["type"], "text");
        assert_eq!(v[1]["type"], "at");
        assert_eq!(v[1]["data"]["qq"], "30011");
        assert_eq!(v[2]["type"], "image");
        assert_eq!(v[2]["data"]["file"], "C:/m/a.png", "图片走本地绝对路径");
    }

    #[test]
    fn 序列化_引用段排在最前() {
        let parts = vec![
            OutgoingPart::Reply { id: "9".into() },
            OutgoingPart::Text { text: "收到".into() },
        ];
        let v = build_outgoing(&parts);
        assert_eq!(v[0]["type"], "reply");
        assert_eq!(v[0]["data"]["id"], "9");
        assert_eq!(v[1]["type"], "text");
    }

    #[test]
    fn 序列化_空文本段被丢弃() {
        let v = build_outgoing(&[OutgoingPart::Text { text: String::new() }]);
        assert_eq!(v.as_array().unwrap().len(), 0);
    }

    #[test]
    fn 校验_空内容不发() {
        assert!(validate_outgoing(&[], true).is_err());
        assert!(validate_outgoing(&[OutgoingPart::Text { text: "   ".into() }], true).is_err());
        assert!(validate_outgoing(&[OutgoingPart::Reply { id: "1".into() }], true).is_err());
    }

    #[test]
    fn 校验_只有令牌没有文本也能发() {
        assert!(
            validate_outgoing(
                &[OutgoingPart::Image { sha256: "x".into(), path: "C:/a.png".into() }],
                true
            )
            .is_ok()
        );
    }

    #[test]
    fn 校验_私聊不允许艾特() {
        let parts = vec![OutgoingPart::At { qq: 1, name: "李工".into() }];
        assert!(validate_outgoing(&parts, false).is_err());
        assert!(validate_outgoing(&parts, true).is_ok());
    }

    #[test]
    fn 会话种子_兼容两种返回形态() {
        // 形态 A：peerUin + chatType
        let a = raw(json!([
            { "peerUin": "30001", "peerName": "大前端交流群", "msgTime": 1700000000, "chatType": 2 }
        ]));
        // 形态 B：user_id / group_id
        let b = raw(json!([
            { "group_id": 30002, "time": 1700000000, "sender": { "nickname": "大林" } }
        ]));
        let merged: Vec<Value> = a.into_iter().chain(b).collect();
        let seed = recent_contact_seed(&merged);

        assert_eq!(seed.len(), 2);
        assert_eq!(seed[0].0, crate::model::Peer::group(30001), "chatType=2 是群");
        assert_eq!(seed[0].1, "大前端交流群");
        assert_eq!(seed[0].2, Some(1_700_000_000_000), "秒级时间戳被补成毫秒");
        assert_eq!(seed[1].0, crate::model::Peer::group(30002));
        assert_eq!(seed[1].1, "大林");
    }

    #[test]
    fn 会话种子_只有_peerUin_且没有类型提示时按私聊处理() {
        let seed = recent_contact_seed(&raw(json!([{ "peerUin": "20001", "peerName": "小陈" }])));
        assert_eq!(seed.len(), 1);
        assert_eq!(seed[0].0, crate::model::Peer::private(20001));
    }

    #[test]
    fn 会话种子_没有名字也不丢() {
        let seed = recent_contact_seed(&raw(json!([{ "user_id": 20002 }])));
        assert_eq!(seed.len(), 1);
        assert_eq!(seed[0].1, "");
        assert_eq!(seed[0].2, None);
    }

    #[test]
    fn 会话种子_跳过非法项() {
        let seed = recent_contact_seed(&raw(json!([{ "foo": "bar" }, { "user_id": 0 }])));
        assert!(seed.is_empty());
    }
}
