use std::sync::Arc;

use crate::core::platform_types::InboundEvent;
use crate::core::scope::ChatScope;
use crate::core::types::ReplyPlan;
use crate::handlers::session_manager::ConversationSessionManager;
use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};
use crate::prelude::XueliResult;

/// 构建好的上下文 — 规划器和 ReplyAgent 的共享输入。
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
    /// 人物事实上下文
    pub person_facts: Option<Vec<String>>,
    /// 长期记忆上下文
    pub memories: Option<Vec<String>>,
    /// 动态记忆（近期相关）
    pub dynamic_memory: Option<String>,
    /// 会话恢复上下文
    pub session_restore: Option<String>,
    /// 精准回忆上下文
    pub precise_recall: Option<String>,
    /// 图片描述上下文
    pub vision_description: Option<String>,
    /// 临时上下文信号（连续性提示）
    pub continuity_hint: Option<String>,
    /// 对话活跃度信号
    pub follows_assistant_recently: bool,
    /// 近期对话消息数（不含当前）
    pub recent_message_count: usize,
}

/// 会话上下文构建器 — 从存储和会话管理器加载历史，构建完整上下文。
pub struct ConversationContextBuilder {
    store: Arc<SqliteConversationStore>,
    session_manager: Option<Arc<ConversationSessionManager>>,
}

impl ConversationContextBuilder {
    pub fn new(store: Arc<SqliteConversationStore>) -> Self {
        Self {
            store,
            session_manager: None,
        }
    }

    /// 设置会话管理器（用于内存会话追踪）
    pub fn with_session_manager(mut self, mgr: Arc<ConversationSessionManager>) -> Self {
        self.session_manager = Some(mgr);
        self
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

        let scope_type = if is_group { "group" } else { "private" };
        let scope_id = scope.group_id().unwrap_or("");

        let stored_records = self.store.get_recent_by_scope(scope_type, scope_id, 20)?;

        let is_first_turn = stored_records.is_empty();
        let recent_message_count = stored_records.len();

        // 格式化近期消息
        let recent_messages: Vec<String> = stored_records
            .iter()
            .map(format_conversation_record)
            .collect();

        // 从会话管理器获取内存消息
        let (session_restore, follows_assistant, continuity) =
            if let Some(ref mgr) = self.session_manager {
                let msgs = mgr.get_recent_messages(&conversation_key, 20).await;
                let restore_text = if msgs.iter().any(|m| m.restored) {
                    Some(format_memory_messages("以下为历史对话记录", &msgs))
                } else {
                    None
                };
                // 检查最近是否有助手发言
                let follows = msgs.last().map(|m| m.role == "assistant").unwrap_or(false);
                // 连续性判断
                let cont = if msgs.len() >= 2 {
                    Some("soft_continuation".to_string())
                } else {
                    Some("unknown".to_string())
                };
                (restore_text, follows, cont)
            } else {
                (None, false, None)
            };

        // 从存储构建动态记忆上下文
        let dynamic_memory = if !stored_records.is_empty() {
            Some(format!(
                "近期对话总计 {} 条，当前为{}聊。",
                stored_records.len(),
                scope_type
            ))
        } else {
            None
        };

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
            dynamic_memory,
            session_restore,
            precise_recall: None,
            vision_description: None,
            continuity_hint: continuity,
            follows_assistant_recently: follows_assistant,
            recent_message_count,
        })
    }
}

impl Default for ConversationContextBuilder {
    fn default() -> Self {
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

/// 格式化历史消息为上下文字符串
fn format_memory_messages(
    header: &str,
    messages: &[crate::handlers::session_manager::MessageEntry],
) -> String {
    let mut lines = vec![header.to_string()];
    for msg in messages {
        let role_tag = if msg.role == "assistant" {
            "助手"
        } else {
            "用户"
        };
        lines.push(format!("{}: {}", role_tag, msg.content));
    }
    lines.join("\n")
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
