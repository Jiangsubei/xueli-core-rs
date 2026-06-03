use crate::core::types::ReplyPlan;
use crate::core::platform_types::InboundEvent;

/// 会话规划器 — 规划回复策略
pub struct ConversationPlanner;

/// 规划结果
#[derive(Debug, Clone)]
pub struct PlanResult {
    pub plan: ReplyPlan,
    pub should_reply: bool,
    pub confidence: f64,
}

impl ConversationPlanner {
    pub fn new() -> Self {
        Self
    }

    /// 规划回复
    pub async fn plan(&self, _event: &InboundEvent) -> Result<PlanResult, String> {
        // TODO: 集成 LLM 调用实现规划
        Ok(PlanResult {
            plan: ReplyPlan {
                id: uuid::Uuid::new_v4().to_string(),
                target_message_id: String::new(),
                topic: None,
                style: None,
                memory_recall_needed: false,
                use_emoji: true,
                priority: 0,
            },
            should_reply: true,
            confidence: 0.8,
        })
    }
}

impl Default for ConversationPlanner {
    fn default() -> Self {
        Self::new()
    }
}