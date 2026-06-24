use crate::core::platform_types::{InboundEvent, SessionRef};
use crate::core::scope::ChatScope;

/// 会话解析器 — 为回复消息确定目标会话。
///
/// 从事件或已有的会话引用中解析出准确的回复目标，统一私聊和群聊的处理差异。
pub struct SessionResolver {
    platform: String,
}

impl SessionResolver {
    pub fn new(platform: &str) -> Self {
        Self {
            platform: platform.to_string(),
        }
    }

    /// 从入站事件直接提取回复所用的会话引用
    pub fn reply_session_for_event(&self, event: &InboundEvent) -> SessionRef {
        event.get_session()
    }

    /// 将目标会话解析为私聊回复目标
    ///
    /// 若目标已是 Private 作用域则直接返回，否则以 user_id 重新构造私聊会话。
    pub fn resolve_private_reply_session(&self, target: &SessionRef) -> SessionRef {
        if target.scope == ChatScope::Private {
            return target.clone();
        }
        let mut session = target.clone();
        session.scope = ChatScope::Private;
        session.session_id = format!(
            "{}:private:{}",
            self.platform,
            target.user_id.as_deref().unwrap_or("")
        );
        session
    }

    /// 将目标会话解析为群聊回复目标
    ///
    /// 若目标非 Group 作用域则自动补全群 ID。可选传入 at_user 覆写回复的用户 ID。
    pub fn resolve_reply_session(&self, target: &SessionRef, at_user: Option<&str>) -> SessionRef {
        let scope = if let ChatScope::Group(ref gid) = target.scope {
            ChatScope::Group(gid.clone())
        } else {
            let fallback_gid = target.user_id.as_deref().unwrap_or("");
            ChatScope::Group(fallback_gid.to_string())
        };
        let group_id = scope.group_id().unwrap_or("").to_string();
        let resolved_user = at_user
            .filter(|u| !u.is_empty())
            .map(|u| u.to_string())
            .or_else(|| target.user_id.clone());
        SessionRef {
            session_id: format!("{}:group:{}", self.platform, group_id),
            scope,
            user_id: resolved_user,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_private_session() {
        let resolver = SessionResolver::new("qq");
        let input = SessionRef {
            session_id: "qq:group:123".into(),
            scope: ChatScope::Group("123".into()),
            user_id: Some("u1".into()),
        };
        let result = resolver.resolve_private_reply_session(&input);
        assert_eq!(result.scope, ChatScope::Private);
        assert_eq!(result.user_id, Some("u1".into()));
        assert!(result.session_id.contains("private"));
    }

    #[test]
    fn test_resolve_private_session_already_private() {
        let resolver = SessionResolver::new("qq");
        let input = SessionRef {
            session_id: "qq:private:u1".into(),
            scope: ChatScope::Private,
            user_id: Some("u1".into()),
        };
        let result = resolver.resolve_private_reply_session(&input);
        assert_eq!(result.scope, ChatScope::Private);
        assert_eq!(result.session_id, "qq:private:u1");
    }

    #[test]
    fn test_resolve_reply_session_with_at_user() {
        let resolver = SessionResolver::new("qq");
        let input = SessionRef {
            session_id: "qq:group:456".into(),
            scope: ChatScope::Group("456".into()),
            user_id: None,
        };
        let result = resolver.resolve_reply_session(&input, Some("at_user_789"));
        assert!(result.scope.is_group());
        assert_eq!(result.user_id, Some("at_user_789".into()));
        assert!(result.session_id.contains("group"));
    }

    #[test]
    fn test_resolve_reply_session_no_at_user() {
        let resolver = SessionResolver::new("qq");
        let input = SessionRef {
            session_id: "qq:group:456".into(),
            scope: ChatScope::Group("456".into()),
            user_id: Some("u1".into()),
        };
        let result = resolver.resolve_reply_session(&input, None);
        assert!(result.scope.is_group());
        assert_eq!(result.user_id, Some("u1".into()));
    }

    #[test]
    fn test_get_session_from_event() {
        let resolver = SessionResolver::new("test");
        let event = InboundEvent {
            id: "e1".into(),
            platform: "test".into(),
            event_type: crate::core::platform_types::EventType::Message,
            message: Some(crate::core::types::UserMessage {
                id: "m1".into(),
                sender_id: "u1".into(),
                sender_name: "tester".into(),
                text: "hello".into(),
                timestamp: chrono::Utc::now(),
                scope: ChatScope::Group("g1".into()),
                is_mention: false,
            }),
            raw_payload: None,
            received_at: chrono::Utc::now(),
            session: None,
            ..Default::default()
        };
        let session = resolver.reply_session_for_event(&event);
        assert!(session.scope.is_group());
        assert_eq!(session.user_id, Some("u1".into()));
    }
}
