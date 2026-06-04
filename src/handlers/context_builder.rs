use std::sync::Arc;

use crate::prelude::XueliResult;
use crate::core::platform_types::InboundEvent;
use crate::core::scope::ChatScope;
use crate::core::types::ReplyPlan;
use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};

/// 构建好的上下文
#[derive(Debug, Clone)]
pub struct ConversationContext {
    /// 当前用户消息文本
    pub user_message: String,
    /// 格式化后的近期消息（从旧到新）
    pub recent_messages: Vec<String>,
    /// 会话标识
    pub conversation_key: String,
    /// 触发用户 ID
    pub user_id: String,
    /// 作用域
    pub scope: ChatScope,
    /// 是否群聊
    pub is_group: bool,
    /// 是否首轮对话
    pub is_first_turn: bool,
    /// 人物事实（可选，ContextBuilder 填充）
    pub person_facts: Option<Vec<String>>,
    /// 长期记忆（可选，ContextBuilder 填充）
    pub memories: Option<Vec<String>>,
}

/// 会话上下文构建器 — 从 ConversationStore 加载历史，构建上下文
pub struct ConversationContextBuilder {
    store: Arc<SqliteConversationStore>,
}

impl ConversationContextBuilder {
    pub fn new(store: Arc<SqliteConversationStore>) -> Self {
        Self { store }
    }

    /// 从事件和回复计划构建上下文
    pub async fn build(
        &self,
        event: &InboundEvent,
        _plan: &ReplyPlan,
    ) -> XueliResult<ConversationContext> {
        let user_message = event
            .message
            .as_ref()
            .map(|m| m.text.clone())
            .unwrap_or_default();

        let user_id = event
            .message
            .as_ref()
            .map(|m| m.sender_id.clone())
            .unwrap_or_default();

        let scope = event
            .message
            .as_ref()
            .map(|m| m.scope.clone())
            .unwrap_or(ChatScope::Private);

        let is_group = scope.is_group();
        let conversation_key = build_conversation_key(&scope, &user_id, &event.platform);

        // 从 store 加载近期消息
        let scope_type = if is_group { "group" } else { "private" };
        let scope_id = scope.group_id().unwrap_or("");

        let stored_records = self.store.get_recent_by_scope(scope_type, scope_id, 20)?;

        let is_first_turn = stored_records.is_empty();

        // 格式化近期消息为 LLM 可读文本
        let recent_messages: Vec<String> = stored_records
            .iter()
            .map(format_conversation_record)
            .collect();

        Ok(ConversationContext {
            user_message,
            recent_messages,
            conversation_key,
            user_id,
            scope,
            is_group,
            is_first_turn,
            person_facts: None,
            memories: None,
        })
    }
}

impl Default for ConversationContextBuilder {
    fn default() -> Self {
        // 使用临时目录作为默认 store（生产环境应从外部注入）
        let dir = std::path::PathBuf::from("data/conversations");
        let store =
            Arc::new(SqliteConversationStore::open(&dir).expect("无法打开默认 ConversationStore"));
        Self::new(store)
    }
}

/// 构建对话标识键
pub fn build_conversation_key(scope: &ChatScope, user_id: &str, platform: &str) -> String {
    let resolved_platform = if platform.is_empty() { "qq" } else { platform };
    match scope {
        ChatScope::Private => format!("{resolved_platform}:private:{user_id}"),
        ChatScope::Group(group_id) => format!("{resolved_platform}:group:{group_id}"),
    }
}

/// 将 ConversationRecord 格式化为一行文本
fn format_conversation_record(record: &ConversationRecord) -> String {
    let role = if record.is_bot {
        "bot"
    } else {
        &record.sender_name
    };
    format!("[{}] {}: {}", record.session_id, role, record.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_conversation_key_private() {
        let key = build_conversation_key(&ChatScope::Private, "user123", "qq");
        assert_eq!(key, "qq:private:user123");
    }

    #[test]
    fn test_build_conversation_key_group() {
        let key = build_conversation_key(&ChatScope::Group("g456".into()), "user123", "qq");
        assert_eq!(key, "qq:group:g456");
    }

    #[test]
    fn test_build_conversation_key_default_platform() {
        let key = build_conversation_key(&ChatScope::Private, "user123", "");
        assert_eq!(key, "qq:private:user123");
    }
}
