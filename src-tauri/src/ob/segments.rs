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

/// 解析上游消息段数组。
///
/// `self_id` 用来判定 @我；@ 的具体昵称留给调用方从成员表补齐
/// （解析阶段不碰数据库，这样这段逻辑可以纯函数测试）。
pub fn parse(raw: &[Value], self_id: i64) -> Parsed {
    let mut out = Parsed::default();

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
#[allow(uncommon_codepoints)]
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
        assert_eq!(p.plain_text(), "@10001 字段名定了没？");
        assert_eq!(p.unknown_at_names(), vec![10001]);
    }

    #[test]
    fn 解析_艾特别人不算艾特我() {
        let p = parse(&raw(json!([{ "type": "at", "data": { "qq": "30011" } }])), 10001);
        assert!(!p.is_at_me);
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

    #[test]
    fn 解析_空段列表降级成占位() {
        let p = parse(&[], 1);
        assert_eq!(p.plain_text(), "[暂不支持的消息类型]");
    }

    #[test]
    fn 序列化_文本艾特图片() {
        let parts = vec![
            OutgoingPart::Text { text: "来了 " },
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
