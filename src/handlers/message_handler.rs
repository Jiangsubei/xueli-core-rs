use std::sync::Arc;

use crate::prelude::XueliResult;
use crate::core::config::XueliConfig;
use crate::core::platform_types::{InboundEvent, ReplyAction};
use crate::handlers::context_builder::ConversationContextBuilder;
use crate::handlers::planner::ConversationPlanner;
use crate::handlers::reply_agent::ReplyAgent;
use crate::handlers::timing_gate::DefaultTimingGate;
use crate::memory::manager::MemoryManager;
use crate::memory::stores::person_fact::SqlitePersonFactStore;
use crate::services::prompt_loader::NoopPromptTemplateLoader;
use crate::traits::ai_client::AIClient;
use crate::traits::platform_adapter::PlatformAdapter;
use crate::traits::prompt_template::PromptTemplateLoader;
use crate::traits::timing_gate::TimingGateStrategy;

/// 消息处理器 — 编排整个回复管线
pub struct MessageHandler<
    A: AIClient,
    P: PlatformAdapter,
    L: PromptTemplateLoader = NoopPromptTemplateLoader,
> {
    config: Arc<XueliConfig>,
    timing_gate: DefaultTimingGate<A, L>,
    planner: ConversationPlanner<A>,
    context_builder: ConversationContextBuilder,
    reply_agent: ReplyAgent<A, L>,
    platform: Arc<P>,
}

impl<A: AIClient, P: PlatformAdapter, L: PromptTemplateLoader> MessageHandler<A, P, L> {
    pub fn new(
        config: Arc<XueliConfig>,
        ai_client: Arc<A>,
        platform: Arc<P>,
        memory_manager: Arc<MemoryManager>,
        person_fact_store: Arc<SqlitePersonFactStore>,
        template_loader: Arc<L>,
        prompt_builder: crate::handlers::prompt_builder::ReplyPromptBuilder<L>,
    ) -> Self {
        let timing_gate = DefaultTimingGate::new(
            ai_client.clone(),
            template_loader,
            config.identity.name.clone(),
            config.identity.alias.clone(),
            "zh-CN",
        );
        let planner_model = config.model.primary_model.clone();
        Self {
            config,
            timing_gate,
            planner: ConversationPlanner::new(ai_client.clone(), &planner_model),
            context_builder: ConversationContextBuilder::default(),
            reply_agent: ReplyAgent::new(
                ai_client,
                memory_manager,
                person_fact_store,
                prompt_builder,
            ),
            platform,
        }
    }

    /// 处理入站事件
    pub async fn handle(&self, event: &InboundEvent) -> XueliResult<Option<ReplyAction>> {
        // 1. Timing Gate
        use crate::traits::timing_gate::TimingContext;
        let ctx = TimingContext {
            event: event.clone(),
            is_mentioned: event
                .message
                .as_ref()
                .map(|m| m.is_mention)
                .unwrap_or(false),
            conversation_active: true,
            time_since_last_reply_secs: 10.0,
            message_count_in_window: 3,
        };

        let decision = self.timing_gate.should_reply(&ctx).await?;
        if !matches!(decision, crate::traits::timing_gate::TimingDecision::Reply) {
            return Ok(None);
        }

        // 2. Context Builder — 加载近期对话上下文
        let context = self
            .context_builder
            .build(
                event,
                &crate::core::types::ReplyPlan {
                    id: String::new(),
                    target_message_id: String::new(),
                    topic: None,
                    style: None,
                    memory_recall_needed: false,
                    use_emoji: true,
                    priority: 0,
                },
            )
            .await?;

        // 3. Planner — 规划回复策略
        let plan = self.planner.plan(event, &context).await?;

        // 4. Reply Agent — 生成回复
        let reply = self
            .reply_agent
            .generate_reply(event, &context, &plan.reply_reference)
            .await?;

        let action = ReplyAction {
            scope: event
                .message
                .as_ref()
                .map(|m| m.scope.clone())
                .unwrap_or(crate::core::scope::ChatScope::Private),
            text: reply.reply_text,
            reply_to: event.message.as_ref().map(|m| m.id.clone()),
            image_url: None,
            emoji_id: None,
        };

        Ok(Some(action))
    }
}
