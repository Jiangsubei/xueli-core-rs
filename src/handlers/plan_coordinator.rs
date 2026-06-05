use std::sync::Arc;

use crate::core::platform_types::InboundEvent;
use crate::core::scope::ChatScope;
use crate::core::types::ReplyPlan;
use crate::handlers::context_builder::ConversationContextBuilder;
use crate::handlers::planner::{ConversationPlanner, PlanResult};
use crate::handlers::session_manager::ConversationSessionManager;
use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};
use crate::prelude::XueliResult;
use crate::traits::ai_client::AIClient;

/// 会话计划协调器 — 协调群聊历史、视觉增强和规划器调用。
///
/// 它是群聊消息处理的核心调度枢纽，负责：构建消息上下文、调用规划器、记录对话历史。
pub struct ConversationPlanCoordinator<A: AIClient> {
    pub planner: Arc<ConversationPlanner<A>>,
    pub session_manager: Arc<ConversationSessionManager>,
    pub context_builder: Arc<ConversationContextBuilder>,
    conversation_store: Option<Arc<SqliteConversationStore>>,
    /// 上下文窗口大小
    context_window_size: usize,
    /// 助手名称
    assistant_name: String,
}

impl<A: AIClient> ConversationPlanCoordinator<A> {
    pub fn new(
        planner: Arc<ConversationPlanner<A>>,
        session_manager: Arc<ConversationSessionManager>,
        context_builder: Arc<ConversationContextBuilder>,
        assistant_name: impl Into<String>,
    ) -> Self {
        Self {
            planner,
            session_manager,
            context_builder,
            conversation_store: None,
            context_window_size: 10,
            assistant_name: assistant_name.into(),
        }
    }

    /// 设置对话存储（用于记录历史）
    pub fn with_conversation_store(mut self, store: Arc<SqliteConversationStore>) -> Self {
        self.conversation_store = Some(store);
        self
    }

    /// 设置助手名称
    pub fn with_assistant_name(mut self, name: &str) -> Self {
        self.assistant_name = name.to_string();
        self
    }

    /// 设置上下文窗口大小
    pub fn with_context_window_size(mut self, size: usize) -> Self {
        self.context_window_size = size;
        self
    }

    /// 主协调入口：构建上下文并调用规划器
    pub async fn coordinate(&self, event: &InboundEvent) -> XueliResult<PlanResult> {
        let conversation_key = self.session_manager.get_key_for_event(event);

        let default_plan = ReplyPlan {
            id: String::new(),
            target_message_id: String::new(),
            topic: None,
            style: None,
            memory_recall_needed: false,
            use_emoji: false,
            priority: 0,
        };
        let context = self.context_builder.build(event, &default_plan).await?;

        let plan_result = self.planner.plan(event, &context).await?;

        tracing::debug!(
            "[计划协调器] conversation={} plan_result={:?}",
            conversation_key,
            plan_result.should_reply
        );

        Ok(plan_result)
    }

    /// 记录助手的回复到对话历史
    pub async fn record_assistant_reply(
        &self,
        event: &InboundEvent,
        reply_text: &str,
    ) -> XueliResult<()> {
        let text = reply_text.trim();
        if text.is_empty() {
            return Ok(());
        }

        let conversation_key = self.session_manager.get_key_for_event(event);
        self.session_manager
            .add_message(&conversation_key, "assistant", text, None, "", "", false)
            .await;

        if let Some(ref store) = self.conversation_store {
            let session_id = conversation_key.clone();
            let sender_name = self.assistant_name.clone();
            let text_owned = text.to_string();
            let is_group = event
                .message
                .as_ref()
                .map(|m| m.scope.is_group())
                .unwrap_or(false);
            let st = if is_group { "group" } else { "private" };
            let sid = event
                .message
                .as_ref()
                .and_then(|m| m.scope.group_id())
                .unwrap_or("")
                .to_string();
            let record = ConversationRecord {
                id: 0,
                session_id,
                user_id: String::new(),
                sender_name,
                text: text_owned,
                is_bot: true,
                scope_type: st.to_string(),
                scope_id: sid,
                event_time: chrono::Utc::now().timestamp() as f64,
                message_id: String::new(),
                platform: event.platform.clone(),
            };
            let _ = store.insert_message(&record);
        }

        Ok(())
    }

    /// 根据事件生成历史键（群聊/私聊的统一格式）
    pub fn history_key(&self, event: &InboundEvent) -> String {
        let session = event.get_session();
        match &session.scope {
            ChatScope::Group(gid) => {
                format!("{}:group:{}", event.platform, gid)
            }
            ChatScope::Private => {
                let uid = session.user_id.as_deref().unwrap_or("");
                format!("{}:private:{}", event.platform, uid)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::platform_types::EventType;
    use crate::core::types::UserMessage;
    use chrono::Utc;

    fn make_event(group_id: &str) -> InboundEvent {
        InboundEvent {
            id: "e1".into(),
            platform: "test".into(),
            event_type: EventType::Message,
            message: Some(UserMessage {
                id: "m1".into(),
                sender_id: "u1".into(),
                sender_name: "测试".into(),
                text: "hello".into(),
                timestamp: Utc::now(),
                scope: ChatScope::Group(group_id.into()),
                is_mention: false,
            }),
            raw_payload: None,
            received_at: Utc::now(),
            session: None,
        }
    }

    #[test]
    fn test_history_key_group() {
        // 创建一个最小化的 coordinator 来测试 history_key
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SqliteConversationStore::open(temp_dir.path()).unwrap());
        let builder = Arc::new(ConversationContextBuilder::new(store));
        let planner = Arc::new(ConversationPlanner::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            "test-model",
        ));
        let session_mgr = Arc::new(ConversationSessionManager::new());

        let coord = ConversationPlanCoordinator::new(planner, session_mgr, builder, "测试助手");

        let event = make_event("g123");
        let key = coord.history_key(&event);
        assert!(key.contains("group"));
        assert!(key.contains("g123"));
    }

    #[test]
    fn test_history_key_private() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SqliteConversationStore::open(temp_dir.path()).unwrap());
        let builder = Arc::new(ConversationContextBuilder::new(store));
        let planner = Arc::new(ConversationPlanner::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            "test-model",
        ));
        let session_mgr = Arc::new(ConversationSessionManager::new());

        let coord = ConversationPlanCoordinator::new(planner, session_mgr, builder, "测试助手");

        let event = InboundEvent {
            id: "e2".into(),
            platform: "test".into(),
            event_type: EventType::Message,
            message: Some(UserMessage {
                id: "m2".into(),
                sender_id: "u2".into(),
                sender_name: "私聊用户".into(),
                text: "hi".into(),
                timestamp: Utc::now(),
                scope: ChatScope::Private,
                is_mention: false,
            }),
            raw_payload: None,
            received_at: Utc::now(),
            session: None,
        };
        let key = coord.history_key(&event);
        assert!(key.contains("private"));
        assert!(key.contains("u2"));
    }
}
