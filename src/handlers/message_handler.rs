use std::sync::Arc;

use crate::core::config::XueliConfig;
use crate::core::message_trace::{build_trace_id, get_execution_key};
use crate::core::platform_types::{InboundEvent, ReplyAction};
use crate::handlers::context_builder::ConversationContextBuilder;
use crate::handlers::plan_coordinator::ConversationPlanCoordinator;
use crate::handlers::planner::ConversationPlanner;
use crate::handlers::prompt_builder::ReplyPromptBuilder;
use crate::handlers::reply_agent::ReplyAgent;
use crate::handlers::session_manager::ConversationSessionManager;
use crate::handlers::timing_gate::DefaultTimingGate;
use crate::memory::manager::MemoryManager;
use crate::memory::stores::person_fact::SqlitePersonFactStore;
use crate::prelude::XueliResult;
use crate::services::prompt_loader::NoopPromptTemplateLoader;
use crate::traits::ai_client::AIClient;
use crate::traits::platform_adapter::PlatformAdapter;
use crate::traits::prompt_template::PromptTemplateLoader;
use crate::traits::timing_gate::{TimingContext, TimingDecision, TimingGateStrategy};

/// 消息处理器 — 编排完整的消息处理管线。
///
/// 执行链：TimingGate → PlanCoordinator → ContextBuilder → Planner → ReplyAgent → ReplyAction
pub struct MessageHandler<
    A: AIClient,
    P: PlatformAdapter,
    L: PromptTemplateLoader = NoopPromptTemplateLoader,
> {
    #[allow(dead_code)]
    config: Arc<XueliConfig>,
    timing_gate: DefaultTimingGate<A, L>,
    plan_coordinator: Arc<ConversationPlanCoordinator<A>>,
    #[allow(dead_code)]
    planner: Arc<ConversationPlanner<A>>,
    reply_agent: ReplyAgent<A, L>,
    session_manager: Arc<ConversationSessionManager>,
    #[allow(dead_code)]
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
        prompt_builder: ReplyPromptBuilder<L>,
    ) -> Self {
        let timing_gate = DefaultTimingGate::new(
            ai_client.clone(),
            template_loader,
            config.identity.name.clone(),
            config.identity.alias.clone(),
            "zh-CN",
        );
        let planner_model = config.model.primary_model.clone();
        let planner = Arc::new(ConversationPlanner::new(ai_client.clone(), &planner_model));
        let session_mgr = Arc::new(ConversationSessionManager::new(None));
        let context_builder = Arc::new(ConversationContextBuilder::default());
        let plan_coord = Arc::new(ConversationPlanCoordinator::new(
            planner.clone(),
            session_mgr.clone(),
            context_builder,
            config.identity.name.clone(),
        ));

        Self {
            config: config.clone(),
            timing_gate,
            plan_coordinator: plan_coord,
            planner,
            reply_agent: ReplyAgent::new(
                config,
                ai_client,
                memory_manager,
                person_fact_store,
                prompt_builder,
            ),
            session_manager: session_mgr,
            platform,
        }
    }

    /// 处理入站事件，返回最终回复（若无回复则返回 None）
    pub async fn handle(&self, event: &InboundEvent) -> XueliResult<Option<ReplyAction>> {
        let trace_id =
            build_trace_id(&event.message.as_ref().map(|m| m.id.as_str()).unwrap_or("0"));
        let execution_key = get_execution_key(event);
        let _session_lock = self.session_manager.get_session_lock(&execution_key).await;
        let _guard = _session_lock.lock().await;

        tracing::info!(
            trace_id = %trace_id,
            execution_key = %execution_key,
            "消息处理开始"
        );

        // 1. Timing Gate — 决定是否回复
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
        if !matches!(decision, TimingDecision::Reply) {
            tracing::debug!(trace_id = %trace_id, "TimingGate 决定不回复");
            return Ok(None);
        }

        // 2. PlanCoordinator — 构建上下文并调用规划器
        let plan_result = self.plan_coordinator.coordinate(event).await?;
        if !plan_result.should_reply {
            tracing::debug!(trace_id = %trace_id, "规划器决定不回复");
            return Ok(None);
        }

        // 3. ContextBuilder — 为 ReplyAgent 准备上下文
        let context = self
            .plan_coordinator
            .context_builder
            .build(event, &plan_result.reply_plan)
            .await?;

        // 4. ReplyAgent — 生成回复文本
        let reply = self
            .reply_agent
            .generate_reply(event, &context, &plan_result.reply_reference)
            .await?;

        // 5. 记录到会话管理器
        let conversation_key = self.session_manager.get_key_for_event(event);
        self.session_manager
            .add_message(
                &conversation_key,
                "assistant",
                &reply.reply_text,
                None,
                "",
                "",
                false,
            )
            .await;

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
