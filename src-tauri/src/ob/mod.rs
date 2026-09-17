//! OneBot 11 适配层。
//!
//! **协议隔离墙**：OneBot 的事件名、字段名、消息段格式只允许出现在这个模块里。
//! 上层只认 `crate::model` 的规范模型，所以 QQ 协议升级时只需要改这里（NFR-13）。
//!
//! 内部一律用 `serde_json::Value` 而不是严格的结构体来读上游——NapCat 的字段在各版本间
//! 有出入（`get_recent_contact` 就有两种返回形态），宽容读取比精确声明更耐用。

pub mod api;
pub mod event;
pub mod model;
pub mod segments;
pub mod ws;
