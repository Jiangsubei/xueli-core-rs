use crate::core::platform_types::InboundEvent;
use crate::core::types::{Conversation, MemoryItem, ReplyPlan};

/// 会话上下文构建器
pub struct ConversationContextBuilder;

/// 构建好的上下文
#[derive(Debug, Clone)]
pub struct ConversationContext {
    pub recent_messages: Vec<String>,
    pub relevant_memories: Vec<MemoryItem>,
    pub conversation_summary: Option<String>,
}

impl ConversationContextBuilder {
    pub fn new() -> Self {
        Self
    }

    pub async fn build(
        &self,
        _event: &InboundEvent,
        _conversation: &Conversation,
        _plan: &ReplyPlan,
    ) -> Result<ConversationContext, String> {
        // TODO: 整合近期消息、检索记忆、会话摘要
        Ok(ConversationContext {
            recent_messages: Vec::new(),
            relevant_memories: Vec::new(),
            conversation_summary: None,
        })
    }
}

impl Default for ConversationContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}