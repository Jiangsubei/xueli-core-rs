/// NapCat / OneBot 11 适配器
///
/// 基于 HTTP POST 上报 + HTTP API 调用的标准 OneBot 11 实现，
/// 兼容 NapCat、go-cqhttp、LLOneBot 等实现。
///
/// 原始事件格式参考：
/// <https://github.com/botuniverse/onebot-11>
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::platform_types::{
    EventType, GroupState, InboundEvent, ReplyAction, SendAction, SessionRef,
};
use crate::core::scope::ChatScope;
use crate::core::types::UserMessage;
use crate::prelude::{XueliError, XueliResult};
use crate::traits::platform_adapter::PlatformAdapter;

/// OneBot 消息段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum OneBotSegment {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "at")]
    At { qq: String },
    #[serde(rename = "image")]
    Image { file: String, url: Option<String> },
    #[serde(rename = "face")]
    Face { id: String },
    #[serde(rename = "reply")]
    Reply { id: String },
}

/// OneBot 11 标准事件（简化版）
#[derive(Debug, Clone, Deserialize)]
pub struct OneBotEvent {
    pub post_type: String,
    #[serde(rename = "message_type")]
    pub message_type: Option<String>,
    #[serde(rename = "sub_type")]
    pub sub_type: Option<String>,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub group_id: String,
    #[serde(default)]
    pub message_id: String,
    pub message: Option<serde_json::Value>,
    #[serde(default)]
    pub raw_message: String,
    #[serde(default)]
    pub sender: Option<OneBotSender>,
    #[serde(default)]
    pub time: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OneBotSender {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub card: String,
}

/// NapCat 适配器配置
#[derive(Debug, Clone)]
pub struct NapCatConfig {
    /// OneBot HTTP API 地址，如 http://127.0.0.1:3000
    pub api_base: String,
    /// 鉴权 Token（Bearer）
    pub access_token: Option<String>,
    /// 机器人自身 QQ 号
    pub self_id: String,
    /// 群聊状态机初始状态
    pub default_group_state: GroupState,
}

impl Default for NapCatConfig {
    fn default() -> Self {
        Self {
            api_base: "http://127.0.0.1:3000".to_string(),
            access_token: None,
            self_id: String::new(),
            default_group_state: GroupState::Running,
        }
    }
}

/// NapCat / OneBot 11 平台适配器
pub struct NapCatAdapter {
    config: NapCatConfig,
    http: reqwest::Client,
}

impl NapCatAdapter {
    pub fn new(config: NapCatConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    /// 发送 OneBot API 请求
    async fn call_api(
        &self,
        action: &str,
        params: serde_json::Value,
    ) -> XueliResult<serde_json::Value> {
        let url = format!("{}/{}", self.config.api_base.trim_end_matches('/'), action);
        let mut req = self.http.post(&url).json(&params);
        if let Some(token) = &self.config.access_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req.send().await.map_err(|e| XueliError::external("http", e.to_string()))?;
        let status = resp.status();
        let body = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| XueliError::external("http", e.to_string()))?;
        if !status.is_success() {
            return Err(XueliError::external(
                "onebot",
                format!("API {} failed: {:?}", action, body),
            ));
        }
        Ok(body)
    }

    /// 将回复动作转换为 OneBot 消息段数组
    fn build_message_segments(&self, action: &ReplyAction) -> Vec<OneBotSegment> {
        let mut segments = Vec::new();

        // 引用回复
        if let Some(ref reply_to) = action.reply_to {
            segments.push(OneBotSegment::Reply {
                id: reply_to.clone(),
            });
        }

        // 文本内容
        if !action.text.is_empty() {
            segments.push(OneBotSegment::Text {
                text: action.text.clone(),
            });
        }

        // 图片
        if let Some(ref url) = action.image_url {
            segments.push(OneBotSegment::Image {
                file: url.clone(),
                url: Some(url.clone()),
            });
        }

        segments
    }

    /// 解析消息段为纯文本（用于提取 @提及）
    fn segments_to_text(segments: &[OneBotSegment]) -> String {
        segments
            .iter()
            .filter_map(|s| match s {
                OneBotSegment::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .concat()
    }

    /// 解析 OneBot 事件为统一 InboundEvent
    fn parse_onebot_event(&self, raw: &str, event: &OneBotEvent) -> XueliResult<InboundEvent> {
        let platform = "napcat".to_string();

        match event.post_type.as_str() {
            "message" => {
                let scope = match event.message_type.as_deref() {
                    Some("group") => ChatScope::Group(event.group_id.clone()),
                    _ => ChatScope::Private,
                };

                let sender_id = event.user_id.clone();
                let sender_name = event
                    .sender
                    .as_ref()
                    .map(|s| {
                        let name = if s.card.is_empty() {
                            &s.nickname
                        } else {
                            &s.card
                        };
                        name.to_string()
                    })
                    .unwrap_or_default();

                let text = if let Some(serde_json::Value::Array(arr)) = &event.message {
                    arr.iter()
                        .filter_map(|v| {
                            if let Ok(seg) = serde_json::from_value::<OneBotSegment>(v.clone()) {
                                match seg {
                                    OneBotSegment::Text { text } => Some(text),
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .concat()
                } else {
                    event.raw_message.clone()
                };

                let is_mention = if let Some(serde_json::Value::Array(arr)) = &event.message {
                    arr.iter().any(|v| {
                        if let Ok(OneBotSegment::At { qq }) =
                            serde_json::from_value::<OneBotSegment>(v.clone())
                        {
                            qq == self.config.self_id
                        } else {
                            false
                        }
                    })
                } else {
                    false
                };

                let event_type = if is_mention {
                    EventType::Mention
                } else {
                    EventType::Message
                };

                let session_id = match &scope {
                    ChatScope::Private => format!("{}:private:{}", platform, sender_id),
                    ChatScope::Group(gid) => format!("{}:group:{}", platform, gid),
                };

                let user_message = UserMessage {
                    id: event.message_id.clone(),
                    sender_id: sender_id.clone(),
                    sender_name,
                    text,
                    scope: scope.clone(),
                    timestamp: chrono::DateTime::from_timestamp(event.time, 0).unwrap_or_else(|| chrono::Utc::now()),
                    is_mention,
                };

                Ok(InboundEvent {
                    id: format!("{}-{}", platform, event.message_id),
                    platform,
                    event_type,
                    message: Some(user_message),
                    raw_payload: Some(raw.to_string()),
                    received_at: chrono::Utc::now(),
                    session: Some(SessionRef {
                        session_id,
                        scope,
                        user_id: Some(sender_id),
                    }),
                })
            }
            "notice" => {
                // 群成员变动等通知事件
                Ok(InboundEvent {
                    id: format!("{}-notice-{}", platform, event.time),
                    platform,
                    event_type: EventType::Other("notice".to_string()),
                    message: None,
                    raw_payload: Some(raw.to_string()),
                    received_at: chrono::Utc::now(),
                    session: None,
                })
            }
            _ => Ok(InboundEvent {
                id: format!("{}-other-{}", platform, event.time),
                platform,
                event_type: EventType::Other(event.post_type.clone()),
                message: None,
                raw_payload: Some(raw.to_string()),
                received_at: chrono::Utc::now(),
                session: None,
            }),
        }
    }
}

#[async_trait]
impl PlatformAdapter for NapCatAdapter {
    async fn send_action(&self, action: &ReplyAction) -> XueliResult<()> {
        let segments = self.build_message_segments(action);

        match &action.scope {
            ChatScope::Private => {
                let user_id = action
                    .scope
                    .user_id()
                    .ok_or_else(|| XueliError::validation("private", "缺少 user_id"))?;
                self.call_api(
                    "send_private_msg",
                    serde_json::json!({
                        "user_id": user_id,
                        "message": segments,
                    }),
                )
                .await?;
            }
            ChatScope::Group(group_id) => {
                self.call_api(
                    "send_group_msg",
                    serde_json::json!({
                        "group_id": group_id,
                        "message": segments,
                    }),
                )
                .await?;
            }
        }

        Ok(())
    }

    fn strip_mentions(&self, text: &str) -> String {
        // OneBot @格式通常为 [CQ:at,qq=xxx] 或纯文本中的 @昵称
        // 这里做简单处理：移除 CQ:at 码和行首的 @昵称
        let re = regex::Regex::new(r"\[CQ:at,qq=\d+\]|^@[^\s]+\s*").unwrap();
        re.replace_all(text, "").trim().to_string()
    }

    fn platform_name(&self) -> &str {
        "napcat"
    }

    fn parse_event(&self, raw: &str) -> XueliResult<InboundEvent> {
        let event: OneBotEvent =
            serde_json::from_str(raw).map_err(|e| XueliError::external("json", e.to_string()))?;
        self.parse_onebot_event(raw, &event)
    }
}

/// 发送动作扩展（支持群状态设置等 NapCat 特有操作）
impl NapCatAdapter {
    /// 设置群聊状态（Running / Waiting / Stopped）
    pub async fn set_group_state(&self, group_id: &str, state: GroupState) -> XueliResult<()> {
        // 实际实现中可通过群文件、数据库或内存状态机记录
        // 这里仅演示 API 调用框架
        tracing::info!(group_id, state = state.as_str(), "设置群聊状态");
        Ok(())
    }

    /// 发送群名片变更请求（NapCat 扩展）
    pub async fn set_group_card(&self, group_id: &str, user_id: &str, card: &str) -> XueliResult<()> {
        self.call_api(
            "set_group_card",
            serde_json::json!({
                "group_id": group_id,
                "user_id": user_id,
                "card": card,
            }),
        )
        .await?;
        Ok(())
    }

    /// 获取群成员列表
    pub async fn get_group_member_list(&self, group_id: &str) -> XueliResult<Vec<serde_json::Value>> {
        let resp = self
            .call_api(
                "get_group_member_list",
                serde_json::json!({ "group_id": group_id }),
            )
            .await?;
        Ok(resp
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_mentions() {
        let adapter = NapCatAdapter::new(NapCatConfig::default());
        assert_eq!(
            adapter.strip_mentions("[CQ:at,qq=12345] 你好"),
            "你好"
        );
        assert_eq!(
            adapter.strip_mentions("@雪梨 你好"),
            "你好"
        );
        assert_eq!(
            adapter.strip_mentions("你好 @雪梨"),
            "你好 @雪梨"
        );
    }

    #[test]
    fn test_parse_private_message() {
        let adapter = NapCatAdapter::new(NapCatConfig {
            self_id: "10000".to_string(),
            ..Default::default()
        });
        let raw = r#"{
            "post_type": "message",
            "message_type": "private",
            "user_id": "12345",
            "message_id": "123456",
            "message": [{"type":"text","data":{"text":"你好"}}],
            "raw_message": "你好",
            "sender": {"user_id":"12345","nickname":"测试用户"},
            "time": 1700000000
        }"#;
        let event = adapter.parse_event(raw).unwrap();
        assert_eq!(event.platform, "napcat");
        assert_eq!(event.event_type, EventType::Message);
        let msg = event.message.unwrap();
        assert_eq!(msg.sender_id, "12345");
        assert_eq!(msg.text, "你好");
        assert!(matches!(msg.scope, ChatScope::Private));
    }

    #[test]
    fn test_parse_group_mention() {
        let adapter = NapCatAdapter::new(NapCatConfig {
            self_id: "10000".to_string(),
            ..Default::default()
        });
        let raw = r#"{
            "post_type": "message",
            "message_type": "group",
            "group_id": "67890",
            "user_id": "12345",
            "message_id": "123456",
            "message": [
                {"type":"at","data":{"qq":"10000"}},
                {"type":"text","data":{"text":" 在吗"}}
            ],
            "raw_message": "[CQ:at,qq=10000] 在吗",
            "sender": {"user_id":"12345","nickname":"测试用户"},
            "time": 1700000000
        }"#;
        let event = adapter.parse_event(raw).unwrap();
        assert_eq!(event.event_type, EventType::Mention);
        let msg = event.message.unwrap();
        assert!(matches!(msg.scope, ChatScope::Group(_)));
        assert_eq!(msg.text, " 在吗");
        assert!(msg.is_mention);
    }

    #[test]
    fn test_build_message_segments() {
        let adapter = NapCatAdapter::new(NapCatConfig::default());
        let action = ReplyAction {
            scope: ChatScope::Private,
            text: "你好".to_string(),
            reply_to: Some("123".to_string()),
            image_url: None,
            emoji_id: None,
        };
        let segs = adapter.build_message_segments(&action);
        assert_eq!(segs.len(), 2);
        assert!(matches!(segs[0], OneBotSegment::Reply { .. }));
        assert!(matches!(segs[1], OneBotSegment::Text { .. }));
    }
}
