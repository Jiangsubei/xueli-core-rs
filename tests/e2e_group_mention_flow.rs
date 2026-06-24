mod common;

use xueli_core::core::config::XueliConfig;
use xueli_core::core::platform_types::{EventType, InboundEvent, ReplyAction};
use xueli_core::core::scope::ChatScope;
use xueli_core::core::types::UserMessage;

/// 创建一条群聊 @ 消息事件
fn make_group_mention_event(text: &str, sender_id: &str) -> InboundEvent {
    InboundEvent {
        id: uuid::Uuid::new_v4().to_string(),
        platform: "mock".to_string(),
        event_type: EventType::Mention,
        message: Some(UserMessage {
            id: uuid::Uuid::new_v4().to_string(),
            sender_id: sender_id.to_string(),
            sender_name: "用户A".to_string(),
            text: text.to_string(),
            timestamp: chrono::Utc::now(),
            scope: ChatScope::Group("group_123".to_string()),
            is_mention: true,
        }),
        mentioned_user_ids: vec!["bot_user_id".to_string()],
        text: text.to_string(),
        ..Default::default()
    }
}

/// 创建一条无 @ 的群聊消息
fn make_group_non_mention_event(text: &str) -> InboundEvent {
    InboundEvent {
        id: uuid::Uuid::new_v4().to_string(),
        platform: "mock".to_string(),
        event_type: EventType::Message,
        message: Some(UserMessage {
            id: uuid::Uuid::new_v4().to_string(),
            sender_id: "user_1".to_string(),
            sender_name: "用户A".to_string(),
            text: text.to_string(),
            timestamp: chrono::Utc::now(),
            scope: ChatScope::Group("group_123".to_string()),
            is_mention: false,
        }),
        text: text.to_string(),
        ..Default::default()
    }
}

#[test]
fn test_group_mention_event_construction() {
    let event = make_group_mention_event("@雪梨 你好", "user_1");
    let msg = event.message.as_ref().unwrap();
    assert!(msg.is_mention);
    assert!(msg.scope.is_group());
    assert_eq!(msg.scope.group_id(), Some("group_123"));
    assert_eq!(msg.text, "@雪梨 你好");
}

#[test]
fn test_group_non_mention_event_construction() {
    let event = make_group_non_mention_event("大家好");
    let msg = event.message.as_ref().unwrap();
    assert!(!msg.is_mention);
    assert!(msg.scope.is_group());
}

#[test]
fn test_mention_reply_probability_config() {
    let config = XueliConfig::default();
    assert_eq!(config.timing_gate.mention_reply_probability, 0.95);
    assert!(config.group_reply.only_reply_when_at);
}

#[test]
fn test_only_reply_when_at_rule() {
    let config = XueliConfig::default();
    assert!(config.group_reply.only_reply_when_at);

    let mention_event = make_group_mention_event("@雪梨 在吗", "user_1");
    assert!(mention_event.message.as_ref().unwrap().is_mention);

    let non_mention_event = make_group_non_mention_event("有人吗");
    assert!(!non_mention_event.message.as_ref().unwrap().is_mention);
}

#[test]
fn test_reply_action_scope_preservation() {
    let action = ReplyAction {
        scope: ChatScope::Group("group_abc".to_string()),
        text: "你好！".to_string(),
        reply_to: Some("msg_001".to_string()),
        image_url: None,
        emoji_id: None,
    };
    assert!(action.scope.is_group());
    assert_eq!(action.scope.group_id(), Some("group_abc"));
    assert_eq!(action.text, "你好！");
}

#[tokio::test]
async fn test_get_session_for_group_mention() {
    let event = make_group_mention_event("@雪梨 测试", "user_1");
    let session = event.get_session();
    assert!(session.session_id.contains("group_123"));
    assert!(session.scope.is_group());
    assert_eq!(session.user_id.as_deref(), Some("user_1"));
}
