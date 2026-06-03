use std::sync::Arc;

use crate::core::config::XueliConfig;
use crate::core::platform_types::{InboundEvent, ReplyAction};
use crate::handlers::timing_gate::DefaultTimingGate;
use crate::handlers::planner::ConversationPlanner;
use crate::handlers::context_builder::ConversationContextBuilder;
use crate::handlers::reply_agent::ReplyAgent;
use crate::traits::ai_client::AIClient;
use crate::traits::platform_adapter::PlatformAdapter;
use crate::traits::timing_gate::TimingGateStrategy;

/// 消息处理器 — 编排整个回复管线
pub struct MessageHandler<A: AIClient, P: PlatformAdapter> {
    config: Arc<XueliConfig>,
    timing_gate: DefaultTimingGate,
    planner: ConversationPlanner,
    context_builder: ConversationContextBuilder,
    reply_agent: ReplyAgent<A>,
    platform: Arc<P>,
}

impl<A: AIClient, P: PlatformAdapter> MessageHandler<A, P> {
    pub fn new(config: Arc<XueliConfig>, ai_client: Arc<A>, platform: Arc<P>) -> Self {
        let timing_gate = DefaultTimingGate::new(Arc::new(config.timing_gate.clone()));
        Self {
            config,
            timing_gate,
            planner: ConversationPlanner::new(),
            context_builder: ConversationContextBuilder::new(),
            reply_agent: ReplyAgent::new(ai_client),
            platform,
        }
    }

    /// 处理入站事件
    pub async fn handle(&self, event: &InboundEvent) -> Result<Option<ReplyAction>, String> {
        // 1. Timing Gate
        use crate::traits::timing_gate::TimingContext;
        let ctx = TimingContext {
            event: event.clone(),
            is_mentioned: event.message.as_ref().map(|m| m.is_mention).unwrap_or(false),
            conversation_active: true,
            time_since_last_reply_secs: 10.0,
            message_count_in_window: 3,
        };

        let decision = self.timing_gate.should_reply(&ctx).await?;
        if !matches!(decision, crate::traits::timing_gate::TimingDecision::Reply) {
            return Ok(None);
        }

        // 2. Planner
        self.planner.plan(event).await?;

        // 3. Context Builder + Reply Agent
        let reply = self.reply_agent.generate_reply(event).await?;

        let action = ReplyAction {
            scope: event
                .message
                .as_ref()
                .map(|m| m.scope.clone())
                .unwrap_or(crate::core::scope::ChatScope::Private),
            text: reply,
            reply_to: event.message.as_ref().map(|m| m.id.clone()),
            image_url: None,
            emoji_id: None,
        };

        Ok(Some(action))
    }
}