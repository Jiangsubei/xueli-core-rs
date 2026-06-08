use crate::core::platform_types::{InboundEvent, SessionRef};

/// 根据消息 ID 构建链路追踪标识
pub fn build_trace_id(message_id: &str) -> String {
    let id = message_id.trim();
    let id = if id.is_empty() { "0" } else { id };
    format!("msg-{}-{}", id, &uuid::Uuid::new_v4().to_string()[..8])
}

/// 获取事件的执行键 — 群聊时追加 user_id，用于按会话/用户维度序列化处理
pub fn get_execution_key(event: &InboundEvent) -> String {
    let session = event.get_session();
    let key = get_execution_key_for_session(&session);
    if event
        .message
        .as_ref()
        .map(|m| m.scope.is_group())
        .unwrap_or(false)
    {
        let user_id = event
            .message
            .as_ref()
            .map(|m| m.sender_id.as_str())
            .unwrap_or("");
        if !user_id.is_empty() {
            return format!("{}:{}", key, user_id);
        }
    }
    key
}

/// 获取会话的执行键 — 群聊用 platform:group:{gid} 格式，私聊直接取 session_id
pub fn get_execution_key_for_session(session: &SessionRef) -> String {
    if session.scope.is_group() {
        let gid = session.scope.group_id().unwrap_or("");
        if !gid.is_empty() {
            // session_id 格式为 "platform:group:{gid}"
            if let Some((platform, _)) = session.session_id.split_once(':') {
                return format!("{}:group:{}", platform, gid);
            }
            return format!("group:{}", gid);
        }
    }
    // 私聊或无 group_id 时，直接返回 session_id
    session.session_id.clone()
}

/// 格式化链路追踪日志行
pub fn format_trace_log(trace_id: &str, session_key: &str, message_id: &str) -> String {
    format!(
        "trace={} session={} message_id={}",
        trace_id, session_key, message_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scope::ChatScope;
    use crate::core::types::UserMessage;
    use chrono::Utc;

    #[test]
    fn test_build_trace_id() {
        let id = build_trace_id("12345");
        assert!(id.starts_with("msg-12345-"));
        assert_eq!(id.len(), 9 + 8 + 1); // "msg-" + id + "-" + 8hex
    }

    #[test]
    fn test_build_trace_id_empty() {
        let id = build_trace_id("");
        assert!(id.starts_with("msg-0-"));
    }

    #[test]
    fn test_get_execution_key_for_session_group() {
        let session = SessionRef {
            session_id: "qq:group:g1".into(),
            scope: ChatScope::Group("g1".into()),
            user_id: Some("u1".into()),
        };
        let key = get_execution_key_for_session(&session);
        assert_eq!(key, "qq:group:g1");
    }

    #[test]
    fn test_get_execution_key_for_session_private() {
        let session = SessionRef {
            session_id: "qq:private:u1".into(),
            scope: ChatScope::Private,
            user_id: Some("u1".into()),
        };
        let key = get_execution_key_for_session(&session);
        assert_eq!(key, "qq:private:u1");
    }

    #[test]
    fn test_get_execution_key_group_with_user() {
        let event = InboundEvent {
            id: "e1".into(),
            platform: "qq".into(),
            event_type: crate::core::platform_types::EventType::Message,
            message: Some(UserMessage {
                id: "m1".into(),
                sender_id: "u_sender".into(),
                sender_name: "T".into(),
                text: "hi".into(),
                timestamp: Utc::now(),
                scope: ChatScope::Group("g1".into()),
                is_mention: true,
            }),
            raw_payload: None,
            received_at: Utc::now(),
            session: None,
            ..Default::default()
        };
        let key = get_execution_key(&event);
        assert!(key.contains("group:g1"));
        assert!(key.contains("u_sender"));
    }

    #[test]
    fn test_format_trace_log() {
        let log_line = format_trace_log("msg-123-abcdef01", "qq:group:g1", "msg_1");
        assert_eq!(
            log_line,
            "trace=msg-123-abcdef01 session=qq:group:g1 message_id=msg_1"
        );
    }
}
