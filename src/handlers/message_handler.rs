use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::Mutex as TokioMutex;
use tracing::{debug, info};

use crate::character::card_service::CharacterCardService;
use crate::character::narrative::NarrativeService;
use crate::core::config::XueliConfig;
use crate::core::context_recorder::ContextRecorder;
use crate::core::drive::engine::DriveEngine;
use crate::core::drive::store::DriveStore;
use crate::core::message_trace::{build_trace_id, get_execution_key};
use crate::core::metrics::RuntimeMetrics;
use crate::core::mood_engine::MoodEngine;
use crate::core::platform_types::{InboundEvent, ReplyAction};
use crate::core::scope::ChatScope;
use crate::core::types::MessageHandlingPlan;
use crate::emoji::database::EmojiDB;
use crate::emoji::manager::EmojiManager;
use crate::emoji::reply_service::{EmojiReplySelection, EmojiReplyService};
use crate::handlers::command::handler::CommandHandler;
use crate::handlers::context_builder::{ConversationContext, ConversationContextBuilder};
use crate::handlers::group_collector::GroupMessageCollector;
use crate::handlers::image_pipeline::ImagePipeline;
use crate::handlers::message_text::MessageTextToolkit;
use crate::handlers::plan_coordinator::ConversationPlanCoordinator;
use crate::handlers::planner::ConversationPlanner;
use crate::handlers::prompt_builder::ReplyPromptBuilder;
use crate::handlers::reply::effect_tracker::ReplyEffectTracker;
use crate::handlers::reply::pipeline::ReplyPipeline;
use crate::handlers::reply::side_effects::ReplySideEffects;
use crate::handlers::reply_agent::{ReplyAgent, ReplyAgentResult};
use crate::handlers::session_manager::ConversationSessionManager;
use crate::handlers::shared::identity_provider::IdentityProvider;
use crate::handlers::timing_gate::DefaultTimingGate;
use crate::memory::flow_service::{MemoryFlowService, MemoryJob};
use crate::memory::manager::MemoryManager;
use crate::memory::stores::mood_store::MoodStore;
use crate::memory::stores::person_fact::SqlitePersonFactStore;
use crate::prelude::XueliResult;
use crate::services::image_client::ImageClient;
use crate::services::token_counter::TokenCounter;
use crate::services::vision_client::VisionClient;
use crate::signals::orchestrator::SignalOrchestrator;
use crate::traits::ai_client::AIClient;
use crate::traits::platform_adapter::PlatformAdapter;
use crate::traits::prompt_template::PromptTemplateLoader;
use crate::traits::timing_gate::{TimingContext, TimingDecision, TimingGateStrategy};

const ACTION_REPLY: &str = "reply";
const ACTION_IGNORE: &str = "ignore";
const ACTION_WAIT: &str = "wait";

/// 待处理的 Wait 决策条目
#[derive(Debug, Clone)]
struct PendingWait {
    event: InboundEvent,
    registered_at: Instant,
    delay: Duration,
    new_messages: Vec<String>,
}

pub struct MessageHandler<
    A: AIClient + 'static,
    P: PlatformAdapter,
    L: PromptTemplateLoader + 'static,
> {
    app_config: Arc<XueliConfig>,
    runtime_metrics: Option<Arc<TokioMutex<RuntimeMetrics>>>,

    ai_client: Arc<A>,
    vision_client: Arc<VisionClient<A, L>>,
    image_client: Arc<ImageClient>,
    token_counter: Arc<TokenCounter>,

    timing_gate: DefaultTimingGate<A, L>,
    conversation_planner: Arc<ConversationPlanner<A, L>>,
    plan_coordinator: Arc<ConversationPlanCoordinator<A, L>>,
    context_builder: Arc<ConversationContextBuilder<L, A>>,
    reply_agent: ReplyAgent<A, L>,
    reply_pipeline: Arc<ReplyPipeline<L>>,

    memory_manager: Arc<MemoryManager<L>>,
    memory_flow_tx: tokio::sync::mpsc::Sender<MemoryJob>,

    character_card_service: Arc<CharacterCardService>,
    narrative_service: Arc<NarrativeService>,

    session_manager: Arc<ConversationSessionManager>,
    context_recorder: Arc<ContextRecorder>,
    group_collector: Arc<GroupMessageCollector>,

    emoji_manager: Arc<EmojiManager<A, L>>,
    emoji_reply_service: Arc<EmojiReplyService<L>>,

    command_handler: Arc<CommandHandler>,

    reply_effect_tracker: Arc<TokioMutex<ReplyEffectTracker>>,
    reply_side_effects: Arc<ReplySideEffects>,

    mood_engines: DashMap<String, MoodEngine>,
    mood_store: Arc<MoodStore>,

    text_toolkit: Arc<MessageTextToolkit>,
    identity_provider: Arc<IdentityProvider<L>>,
    image_pipeline: Arc<ImagePipeline<A, L>>,

    last_send_time: DashMap<String, Instant>,
    pending_waits: DashMap<String, PendingWait>,
    rate_limit_lock: TokioMutex<()>,

    platform: Arc<P>,
}

impl<A: AIClient + 'static, P: PlatformAdapter, L: PromptTemplateLoader + 'static>
    MessageHandler<A, P, L>
{
    pub fn new(
        config: Arc<XueliConfig>,
        ai_client: Arc<A>,
        platform: Arc<P>,
        memory_manager: Arc<MemoryManager<L>>,
        person_fact_store: Arc<SqlitePersonFactStore>,
        template_loader: Arc<L>,
        prompt_builder: ReplyPromptBuilder<L>,
    ) -> Self {
        let token_counter = Arc::new(
            TokenCounter::new_cl100k()
                .unwrap_or_else(|_| TokenCounter::new_o200k().expect("TokenCounter 创建失败")),
        );
        let image_client = match ImageClient::new() {
            Ok(c) => Arc::new(c),
            Err(_) => Arc::new(ImageClient::new().expect("ImageClient 创建失败")),
        };
        let vision_client = Arc::new(VisionClient::new(
            config.clone(),
            ai_client.clone(),
            template_loader.clone(),
            "zh-CN",
        ));

        let timing_gate = DefaultTimingGate::new(
            ai_client.clone(),
            template_loader.clone(),
            config.identity.name.clone(),
            config.identity.alias.clone(),
            "zh-CN",
        );

        let planner_model = config.model.primary_model.clone();
        let planner = Arc::new(
            ConversationPlanner::new(
                ai_client.clone(),
                template_loader.clone(),
                &planner_model,
                &config.identity.name,
                &config.identity.alias,
                "zh-CN",
            )
            .with_emoji_enabled(config.emoji.enabled),
        );
        let session_mgr = Arc::new(ConversationSessionManager::new(Some(
            memory_manager.conversation_store(),
        )));

        let reply_agent = ReplyAgent::new(
            config.clone(),
            ai_client.clone(),
            memory_manager.clone(),
            person_fact_store,
            prompt_builder,
        );

        let reply_pipeline = Arc::new(ReplyPipeline::new(
            config.clone(),
            Some(memory_manager.clone()),
            Some(memory_manager.conversation_store()),
        ));

        let (flow_service, mut memory_flow_rx) =
            MemoryFlowService::new(memory_manager.clone(), None, None);
        let memory_flow_tx = flow_service.tx.clone();
        {
            tokio::spawn(async move {
                flow_service.run(&mut memory_flow_rx).await;
            });
        }

        let db_dir = std::path::Path::new(&config.memory.data_dir);
        let emoji_db = EmojiDB::new(&db_dir.to_string_lossy());
        let emoji_manager =
            Arc::new(EmojiManager::new(emoji_db).with_vision_client(vision_client.clone()));

        let emoji_database = Arc::new(
            crate::emoji::database::EmojiDatabase::new(db_dir.join("emojis.db")).unwrap_or_else(
                |_| {
                    let tmp = std::env::temp_dir().join("xueli_emoji.db");
                    crate::emoji::database::EmojiDatabase::new(&tmp)
                        .expect("EmojiDatabase 创建失败")
                },
            ),
        );
        let emoji_reply_service = Arc::new(EmojiReplyService::new(
            config.emoji.clone(),
            ai_client.clone(),
            emoji_database,
            template_loader.clone(),
            config.model.primary_model.clone(),
        ));

        let reply_effect_tracker = Arc::new(TokioMutex::new(ReplyEffectTracker::new(600.0)));
        let signal_orchestrator = Arc::new(SignalOrchestrator::new(
            memory_manager.signal_store(),
            ai_client.clone(),
            template_loader.clone(),
            &config.model.primary_model,
            "zh-CN",
            60.0,
            "v1",
            "global",
        ));

        let text_toolkit = Arc::new(MessageTextToolkit::new(
            config.bot_behavior.max_message_length,
        ));

        let identity_provider = Arc::new(IdentityProvider::new(
            config.clone(),
            template_loader.clone(),
            "zh-CN",
        ));

        let image_pipeline = Arc::new(ImagePipeline::new(
            vision_client.clone(),
            image_client.clone(),
            Some(emoji_manager.clone()),
        ));

        let drive_store = DriveStore::new(&config.memory.data_dir);
        let drive_engine = Arc::new(DriveEngine::new(drive_store, "global", true));

        let context_builder = ConversationContextBuilder::new(memory_manager.conversation_store())
            .with_session_manager(session_mgr.clone())
            .with_memory_manager(memory_manager.clone())
            .with_retrieval_coordinator(memory_manager.retrieval_coordinator())
            .with_drive_engine(drive_engine.clone())
            .with_image_pipeline(image_pipeline.clone());
        let context_builder = Arc::new(context_builder);

        let plan_coord = ConversationPlanCoordinator::new(
            planner.clone(),
            session_mgr.clone(),
            context_builder.clone(),
            config.identity.name.clone(),
        )
        .with_conversation_store(memory_manager.conversation_store());
        let plan_coord = Arc::new(plan_coord);

        let group_collector = Arc::new(
            GroupMessageCollector::new(50)
                .with_conversation_store(memory_manager.conversation_store())
                .with_bot_name(&config.identity.name),
        );

        let context_recorder = Arc::new(ContextRecorder::new(Some(
            db_dir.join("xueli_memory.db").to_string_lossy().to_string(),
        )));

        let character_card_service = {
            let storage_dir = db_dir.join("_character_cards");
            let storage_dir_str = storage_dir.to_string_lossy().to_string();
            let card = CharacterCardService::default_card();
            Arc::new(CharacterCardService::new(card, &storage_dir_str))
        };
        let narrative_service = {
            let storage_dir = db_dir.join("_narrative_threads");
            let storage_dir_str = storage_dir.to_string_lossy().to_string();
            Arc::new(NarrativeService::new(&storage_dir_str))
        };

        let reply_side_effects = Arc::new(
            ReplySideEffects::new(reply_effect_tracker.clone())
                .with_signal_orchestrator(signal_orchestrator)
                .with_character_card_service(character_card_service.clone()),
        );

        let command_handler = Arc::new(CommandHandler::new(
            config.clone(),
            session_mgr.clone(),
            None,
            None,
        ));

        let mood_store = {
            let mood_path = db_dir.join("moods.db");
            Arc::new(MoodStore::new(&mood_path).unwrap_or_else(|_| {
                let tmp = std::env::temp_dir().join("xueli_moods.db");
                MoodStore::new(&tmp).expect("MoodStore 创建失败")
            }))
        };

        Self {
            app_config: config,
            runtime_metrics: None,
            ai_client,
            vision_client,
            image_client,
            token_counter,
            timing_gate,
            conversation_planner: planner,
            plan_coordinator: plan_coord,
            context_builder,
            reply_agent,
            reply_pipeline,
            memory_manager,
            memory_flow_tx,
            character_card_service,
            narrative_service,
            session_manager: session_mgr,
            context_recorder,
            group_collector,
            emoji_manager,
            emoji_reply_service,
            command_handler,
            reply_effect_tracker,
            reply_side_effects,
            mood_engines: DashMap::new(),
            mood_store,
            text_toolkit,
            identity_provider,
            image_pipeline,
            last_send_time: DashMap::new(),
            pending_waits: DashMap::new(),
            rate_limit_lock: TokioMutex::new(()),
            platform,
        }
    }

    pub async fn handle(&self, event: &InboundEvent) -> XueliResult<Option<ReplyAction>> {
        let trace_id =
            build_trace_id(&event.message.as_ref().map(|m| m.id.as_str()).unwrap_or("0"));
        let execution_key = get_execution_key(event);
        let _session_lock = self.session_manager.get_session_lock(&execution_key).await;
        let _guard = _session_lock.lock().await;

        let start = Instant::now();

        info!(
            trace_id = %trace_id,
            execution_key = %execution_key,
            "消息处理开始"
        );

        // 记录收到消息指标
        self._record_metrics_message_received();

        // 收集群聊消息到缓冲区
        let _ = self.collect_group_message(event).await;

        let plan = self.plan_message(event, &trace_id).await?;

        if !plan.should_reply {
            debug!(trace_id = %trace_id, reason = %plan.reason, "消息处理计划决定不回复");
            self._record_metrics_ignored(&plan.action);
            return Ok(None);
        }

        // 速率限制检查
        let target_id = event
            .message
            .as_ref()
            .map(|m| m.sender_id.clone())
            .unwrap_or_default();
        if !target_id.is_empty() && !self.check_rate_limit(&target_id).await {
            return Ok(None);
        }

        let reply_action = self
            .get_ai_response(event, Some(&plan), None, &trace_id)
            .await?;

        if let Some(ref action) = reply_action {
            self.record_reply_sent(event, &action.text, Some(&plan))
                .await;

            // 更新叙事线
            self._update_narrative_thread(event, &action.text).await;

            // 持久化心情状态
            self._persist_mood_state(event).await;

            // 记录回复发送指标
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
            self._record_metrics_reply_sent(elapsed_ms);
        }

        Ok(reply_action)
    }

    pub async fn plan_message(
        &self,
        event: &InboundEvent,
        trace_id: &str,
    ) -> XueliResult<MessageHandlingPlan> {
        let empty_plan = || MessageHandlingPlan {
            action: String::new(),
            reason: String::new(),
            source: String::new(),
            should_reply: false,
            raw_decision: None,
            reply_context: HashMap::new(),
            prompt_plan: None,
            reply_reference: String::new(),
            planning_signals: HashMap::new(),
            planner_caution_hint: None,
            risk_posture: None,
            cached_context: None,
        };

        if self._is_self_message(event) {
            let mut p = empty_plan();
            p.action = ACTION_IGNORE.to_string();
            p.reason = "机器人自己的消息，跳过处理".to_string();
            p.source = "rule".to_string();
            return Ok(p);
        }

        let scope = event
            .message
            .as_ref()
            .map(|m| m.scope.clone())
            .unwrap_or(ChatScope::Private);

        // 先检查是否有待处理的 wait 决策已解决
        let conversation_key = self.session_manager.get_key_for_event(event);
        if let Some(decision) = self.take_wait_decision(&conversation_key).await {
            match decision {
                TimingDecision::Reply => {
                    return self._plan_with_coordinator(event, trace_id).await;
                }
                TimingDecision::Wait(delay) => {
                    let mut p = empty_plan();
                    p.action = ACTION_WAIT.to_string();
                    p.reason = format!("Wait 重入后仍决定等待 {:.0} 秒", delay);
                    p.source = "timing_gate".to_string();
                    return Ok(p);
                }
                TimingDecision::Ignore => {
                    let mut p = empty_plan();
                    p.action = ACTION_IGNORE.to_string();
                    p.reason = "Wait 重入后决定忽略".to_string();
                    p.source = "timing_gate".to_string();
                    return Ok(p);
                }
            }
        }

        // 新消息到达时通知 TimingGate 的 wait 队列
        let user_message = self.extract_user_message(event);
        self.notify_new_message(&conversation_key, &user_message);

        if scope.is_private() {
            return self._plan_with_coordinator(event, trace_id).await;
        }

        let only_at_mode = self.app_config.group_reply.only_reply_when_at;

        if only_at_mode {
            if self._is_direct_mention(event) {
                return self._plan_with_coordinator(event, trace_id).await;
            }
            let mut p = empty_plan();
            p.action = ACTION_IGNORE.to_string();
            p.reason = "群聊仅在被 @ 时回复，跳过未 @ 消息".to_string();
            p.source = "rule".to_string();
            return Ok(p);
        }

        if self._is_direct_mention(event) {
            return self._plan_with_coordinator(event, trace_id).await;
        }

        let timing_plan = self._plan_with_timing_gate(event, trace_id).await?;
        if timing_plan.action == ACTION_REPLY {
            return self._plan_with_coordinator(event, trace_id).await;
        }

        // Wait 决策：注册到 TimingGate 等待队列
        if timing_plan.action == ACTION_WAIT {
            if let Some(delay) = timing_plan
                .reply_context
                .get("wait_delay_seconds")
                .and_then(|v| v.as_f64())
            {
                self.register_wait(&conversation_key, event.clone(), delay);
            }
        }

        Ok(timing_plan)
    }

    /// 注册 Wait 决策到待处理队列
    fn register_wait(&self, conversation_key: &str, event: InboundEvent, delay_seconds: f64) {
        let delay = Duration::from_secs_f64(delay_seconds.max(0.05));
        self.pending_waits.insert(
            conversation_key.to_string(),
            PendingWait {
                event,
                registered_at: Instant::now(),
                delay,
                new_messages: Vec::new(),
            },
        );
    }

    /// 新消息到达时通知待处理 Wait 队列
    fn notify_new_message(&self, conversation_key: &str, user_message: &str) {
        if let Some(mut wait) = self.pending_waits.get_mut(conversation_key) {
            wait.new_messages.push(user_message.to_string());
        }
    }

    /// 取出并执行 Wait 决策的二次判断
    ///
    /// - 若等待未到期，返回剩余等待时间
    /// - 若等待已到期，移除该等待项并使用 TimingGate 重新评估
    async fn take_wait_decision(&self, conversation_key: &str) -> Option<TimingDecision> {
        let (expired, event, new_messages, remaining) = {
            let wait = self.pending_waits.get(conversation_key)?;
            let elapsed = wait.registered_at.elapsed();
            if elapsed < wait.delay {
                (false, wait.event.clone(), Vec::new(), wait.delay - elapsed)
            } else {
                (
                    true,
                    wait.event.clone(),
                    wait.new_messages.clone(),
                    Duration::ZERO,
                )
            }
        };

        if !expired {
            return Some(TimingDecision::Wait(remaining.as_secs_f64()));
        }

        self.pending_waits.remove(conversation_key);

        let mut ctx = self._build_timing_context(&event).await;
        ctx.new_messages_since_wait = new_messages;

        match self.timing_gate.should_reply(&ctx).await {
            Ok(decision) => Some(decision),
            Err(_) => Some(TimingDecision::Ignore),
        }
    }

    async fn _plan_with_coordinator(
        &self,
        event: &InboundEvent,
        _trace_id: &str,
    ) -> XueliResult<MessageHandlingPlan> {
        match self.plan_coordinator.coordinate(event).await {
            Ok(plan) => Ok(plan),
            Err(e) => {
                tracing::warn!("[MessageHandler] 规划协调失败，使用规则 fallback: {}", e);
                let mut p = MessageHandlingPlan {
                    action: String::new(),
                    reason: String::new(),
                    source: String::new(),
                    should_reply: false,
                    raw_decision: None,
                    reply_context: HashMap::new(),
                    prompt_plan: None,
                    reply_reference: String::new(),
                    planning_signals: HashMap::new(),
                    planner_caution_hint: None,
                    risk_posture: None,
                    cached_context: None,
                };
                p.action = ACTION_REPLY.to_string();
                p.reason = "规划器失败，规则 fallback 回复".to_string();
                p.source = "rule_fallback".to_string();
                p.should_reply = true;
                Ok(p)
            }
        }
    }

    async fn _plan_with_timing_gate(
        &self,
        event: &InboundEvent,
        _trace_id: &str,
    ) -> XueliResult<MessageHandlingPlan> {
        let ctx = self._build_timing_context(event).await;

        let decision = self.timing_gate.should_reply(&ctx).await;

        let (action, reason, wait_delay) = match decision {
            Ok(d) => {
                let (action, delay) = match d {
                    TimingDecision::Reply => (ACTION_REPLY, None),
                    TimingDecision::Wait(secs) => (ACTION_WAIT, Some(secs)),
                    TimingDecision::Ignore => (ACTION_IGNORE, None),
                };
                (action.to_string(), "Time Gate 评估结果".to_string(), delay)
            }
            Err(_) => (
                ACTION_IGNORE.to_string(),
                "Time Gate 评估失败，跳过".to_string(),
                None,
            ),
        };

        let should_reply = action == ACTION_REPLY;

        let mut reply_context = HashMap::new();
        if let Some(delay) = wait_delay {
            reply_context.insert("wait_delay_seconds".to_string(), serde_json::json!(delay));
        }

        Ok(MessageHandlingPlan {
            action,
            reason,
            source: "timing_gate".to_string(),
            should_reply,
            raw_decision: None,
            reply_context,
            prompt_plan: None,
            reply_reference: String::new(),
            planning_signals: HashMap::new(),
            planner_caution_hint: None,
            risk_posture: None,
            cached_context: None,
        })
    }

    async fn _build_timing_context(&self, event: &InboundEvent) -> TimingContext {
        let conversation_key = self.session_manager.get_key_for_event(event);
        let recent_records = self
            .session_manager
            .get_recent_messages(&conversation_key, 20)
            .await;

        let recent_history_text = if recent_records.is_empty() {
            String::new()
        } else {
            let items: Vec<crate::handlers::shared::unified_history_renderer::UnifiedHistoryItem> =
                recent_records
                    .iter()
                    .map(|m| {
                        crate::handlers::shared::unified_history_renderer::UnifiedHistoryItem {
                            timestamp: m.timestamp,
                            role: m.role.clone(),
                            content: m.content.clone(),
                        }
                    })
                    .collect();
            crate::handlers::shared::unified_history_renderer::render_unified_history(
                &items, "", false, 50,
            )
        };

        let last_reply_time = recent_records
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.timestamp);
        let time_since_last_reply_secs = match last_reply_time {
            Some(t) => (chrono::Utc::now().timestamp() as f64 - t).max(0.0),
            None => 3600.0,
        };

        let message_count_in_window = recent_records.len() as u32;

        TimingContext {
            event: event.clone(),
            is_mentioned: self._is_direct_mention(event),
            conversation_active: message_count_in_window > 0,
            time_since_last_reply_secs,
            message_count_in_window,
            recent_history_text,
            new_messages_since_wait: Vec::new(),
        }
    }

    pub async fn build_message_context(
        &self,
        event: &InboundEvent,
        plan: Option<&MessageHandlingPlan>,
        _trace_id: &str,
        _include_memory: bool,
    ) -> XueliResult<ConversationContext> {
        let reply_plan = plan
            .and_then(|p| {
                p.reply_context.get("_reply_plan").and_then(|v| {
                    serde_json::from_value::<crate::core::types::ReplyPlan>(v.clone()).ok()
                })
            })
            .unwrap_or_else(|| crate::core::types::ReplyPlan {
                id: String::new(),
                target_message_id: String::new(),
                topic: None,
                style: None,
                memory_recall_needed: false,
                use_emoji: false,
                priority: 0,
            });

        self.context_builder.build(event, &reply_plan).await
    }

    pub async fn get_ai_response(
        &self,
        event: &InboundEvent,
        plan: Option<&MessageHandlingPlan>,
        prebuilt_context: Option<ConversationContext>,
        trace_id: &str,
    ) -> XueliResult<Option<ReplyAction>> {
        let user_message = self.extract_user_message(event);

        if let Some(cmd_text) = self.command_handler.handle(&user_message, event) {
            let scope = event
                .message
                .as_ref()
                .map(|m| m.scope.clone())
                .unwrap_or(ChatScope::Private);
            return Ok(Some(ReplyAction {
                scope,
                text: cmd_text,
                reply_to: event.message.as_ref().map(|m| m.id.clone()),
                image_url: None,
                emoji_id: None,
            }));
        }

        // 视觉分析现在由 ContextBuilder 在 build() 中统一注入
        let context = match prebuilt_context {
            Some(ctx) => ctx,
            None => {
                self.build_message_context(event, plan, trace_id, true)
                    .await?
            }
        };

        let reply_reference = plan.map(|p| p.reply_reference.as_str()).unwrap_or("");

        let result: ReplyAgentResult = self
            .reply_agent
            .generate_reply(event, &context, reply_reference)
            .await?;

        let scope = event
            .message
            .as_ref()
            .map(|m| m.scope.clone())
            .unwrap_or(ChatScope::Private);

        let conversation_key = self.session_manager.get_key_for_event(event);
        self.session_manager
            .add_message(
                &conversation_key,
                "assistant",
                &result.reply_text,
                None,
                "",
                "",
                false,
            )
            .await;

        let _ = self
            .memory_flow_tx
            .send(MemoryJob::RegisterDialogue {
                user_id: event
                    .message
                    .as_ref()
                    .map(|m| m.sender_id.clone())
                    .unwrap_or_default(),
                user_message: context.user_message.clone(),
                assistant_message: result.reply_text.clone(),
                dialogue_key: conversation_key.clone(),
                scope_type: if scope.is_group() {
                    "group".to_string()
                } else {
                    "private".to_string()
                },
                group_id: scope.group_id().unwrap_or("").to_string(),
                message_id: event
                    .message
                    .as_ref()
                    .map(|m| m.id.clone())
                    .unwrap_or_default(),
                image_description: context.vision_description.unwrap_or_default(),
                narrative_summary: context.narrative_thread_summary.clone().unwrap_or_default(),
                platform: event.platform.clone(),
                warmth_guidance: String::new(),
                user_emotion_label: String::new(),
                intimacy_delta: 0.0,
            })
            .await;
        self._apply_mood_adjustments(event, &result.reply_text)
            .await;

        Ok(Some(ReplyAction {
            scope,
            text: result.reply_text,
            reply_to: event.message.as_ref().map(|m| m.id.clone()),
            image_url: None,
            emoji_id: None,
        }))
    }

    pub async fn record_reply_sent(
        &self,
        event: &InboundEvent,
        text: &str,
        plan: Option<&MessageHandlingPlan>,
    ) {
        let scope = event
            .message
            .as_ref()
            .map(|m| m.scope.clone())
            .unwrap_or(ChatScope::Private);

        let conversation_key = self.session_manager.get_key_for_event(event);
        self.session_manager
            .add_message(&conversation_key, "assistant", text, None, "", "", false)
            .await;

        let expected_effect = plan
            .and_then(|p| {
                p.reply_context
                    .get("expected_effect")
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();
        let predicted_response = plan
            .and_then(|p| {
                p.reply_context
                    .get("predicted_user_response")
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();

        {
            let mut tracker = self.reply_effect_tracker.lock().await;
            tracker.record_reply(
                &event
                    .message
                    .as_ref()
                    .map(|m| m.sender_id.clone())
                    .unwrap_or_default(),
                scope.group_id().unwrap_or(""),
                text,
                &expected_effect,
                &expected_effect,
                &predicted_response,
            );
        }
    }

    pub async fn check_rate_limit(&self, target_id: &str) -> bool {
        let interval = self.app_config.bot_behavior.rate_limit_interval;
        let _lock = self.rate_limit_lock.lock().await;
        let last_time = self.last_send_time.get(target_id).map(|t| *t.value());
        if let Some(last) = last_time {
            let elapsed = last.elapsed();
            if elapsed.as_secs_f64() < interval {
                tracing::debug!("[RATE LIMIT] {} 被限流，跳过回复", target_id);
                return false;
            }
        }
        self.last_send_time
            .insert(target_id.to_string(), Instant::now());
        true
    }

    pub async fn plan_emoji_follow_up(
        &self,
        event: &InboundEvent,
        reply_text: &str,
        plan: Option<&MessageHandlingPlan>,
    ) -> Option<EmojiReplySelection> {
        if reply_text.trim().is_empty() {
            return None;
        }
        let user_message = self.extract_user_message(event);
        let trace_id = plan
            .and_then(|p| p.reply_context.get("trace_id").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        let empty_ctx = HashMap::new();
        let reply_ctx = plan.map(|p| &p.reply_context).unwrap_or(&empty_ctx);

        self.emoji_reply_service
            .plan_follow_up(
                event,
                &user_message,
                reply_text,
                reply_ctx,
                &trace_id,
                "",
                None,
            )
            .await
            .ok()
            .flatten()
    }

    pub fn build_emoji_follow_up_action(
        &self,
        selection: &EmojiReplySelection,
        session: &crate::core::platform_types::SessionRef,
    ) -> Option<ReplyAction> {
        self.emoji_reply_service
            .build_follow_up_action(selection, session)
    }

    pub async fn mark_emoji_follow_up_sent(
        &self,
        event: &InboundEvent,
        selection: &EmojiReplySelection,
    ) {
        let _ = self
            .emoji_reply_service
            .mark_follow_up_sent(event, selection)
            .await;
    }

    /// 应用心情引擎 — 记录回复作为心情增量
    ///
    /// 读取当前 scope 的心情状态，应用正向增量并返回可见提示。
    pub async fn _apply_mood_adjustments(&self, event: &InboundEvent, reply_text: &str) {
        let key = self._scope_mood_key(event);
        let _ = self._get_or_create_mood_engine(&key);

        if let Some(mut engine) = self.mood_engines.get_mut(&key) {
            let reply_len = reply_text.trim().len() as f64;
            let valence_delta = if reply_len > 20.0 {
                Some(0.05)
            } else if reply_len > 0.0 {
                Some(0.02)
            } else {
                None
            };

            let deltas = crate::core::mood_engine::MoodDeltas {
                valence_delta,
                energy_delta: Some(-0.03),
                arousal_delta: if reply_len > 50.0 { Some(0.04) } else { None },
            };
            engine.apply_deltas(&deltas);
        }
    }

    /// 记录收到消息到运行时指标
    fn _record_metrics_message_received(&self) {
        if let Some(ref metrics) = self.runtime_metrics {
            if let Ok(m) = metrics.try_lock() {
                m.record_message_received();
            }
        }
    }

    /// 记录忽略决策到运行时指标
    fn _record_metrics_ignored(&self, action: &str) {
        if let Some(ref metrics) = self.runtime_metrics {
            if let Ok(m) = metrics.try_lock() {
                m.record_planner_action(action);
                if action == ACTION_IGNORE {
                    m.record_ignored();
                }
            }
        }
    }

    /// 记录回复发送到运行时指标
    fn _record_metrics_reply_sent(&self, latency_ms: f64) {
        if let Some(ref metrics) = self.runtime_metrics {
            if let Ok(m) = metrics.try_lock() {
                m.record_reply_sent(latency_ms);
            }
        }
    }

    /// 更新叙事线 — 记录交互事件
    async fn _update_narrative_thread(&self, event: &InboundEvent, reply_text: &str) {
        let user_id = event
            .message
            .as_ref()
            .map(|m| m.sender_id.clone())
            .unwrap_or_default();
        if user_id.is_empty() {
            return;
        }
        let user_message = self.extract_user_message(event);
        if !user_message.is_empty() {
            self.narrative_service.add_event(
                &user_id,
                &format!(
                    "用户: {} → 助手: {}",
                    truncate_str(&user_message, 40),
                    truncate_str(reply_text, 40)
                ),
                0.3,
            );
        }
    }

    /// 持久化心情状态到 MoodStore
    async fn _persist_mood_state(&self, event: &InboundEvent) {
        let key = self._scope_mood_key(event);
        if let Some(engine) = self.mood_engines.get(&key) {
            if let Some(state) = engine.current() {
                let _ = self.mood_store.save_async(&key, state).await;
            }
        }
    }

    /// 从 MoodStore 恢复心情状态
    pub async fn restore_mood_state(&self, event: &InboundEvent) {
        let key = self._scope_mood_key(event);
        let state = self.mood_store.load_async(&key).await;
        self._get_or_create_mood_engine(&key);
        if let Some(mut engine) = self.mood_engines.get_mut(&key) {
            engine.load(state);
        }
    }

    /// 获取角色卡快照
    pub fn get_character_card_snapshot(
        &self,
        user_id: &str,
    ) -> crate::character::card_service::CharacterCardSnapshot {
        self.character_card_service.get_snapshot(user_id)
    }

    /// 获取叙事线程摘要
    pub fn get_narrative_summary(&self, user_id: &str) -> String {
        let thread = self.narrative_service.get_thread(user_id);
        if thread.summary.is_empty() {
            String::new()
        } else {
            thread.summary
        }
    }

    /// 构建身份文本（用于提示词注入）
    pub async fn build_identity_text(&self) -> String {
        self.identity_provider.build_identity_text().await
    }

    /// 检查视觉客户端是否可用
    pub fn is_vision_available(&self) -> bool {
        self.vision_client.is_available()
    }

    /// 获取 token 计数
    pub fn count_tokens(&self, text: &str) -> usize {
        self.token_counter.count(text)
    }

    /// 获取 AI 客户端引用（供外部调用使用）
    pub fn ai_client(&self) -> &Arc<A> {
        &self.ai_client
    }

    /// 获取规划器引用
    pub fn planner(&self) -> &Arc<ConversationPlanner<A, L>> {
        &self.conversation_planner
    }

    /// 获取规划协调器引用
    pub fn plan_coordinator(&self) -> &Arc<ConversationPlanCoordinator<A, L>> {
        &self.plan_coordinator
    }

    /// 获取回复管线引用
    pub fn reply_pipeline(&self) -> &Arc<ReplyPipeline<L>> {
        &self.reply_pipeline
    }

    /// 获取记忆管理器引用
    pub fn memory_manager(&self) -> &Arc<MemoryManager<L>> {
        &self.memory_manager
    }

    /// 获取表情管理器引用
    pub fn emoji_manager(&self) -> &Arc<EmojiManager<A, L>> {
        &self.emoji_manager
    }

    /// 获取平台适配器引用
    pub fn platform(&self) -> &Arc<P> {
        &self.platform
    }

    /// 获取图片客户端引用
    pub fn image_client(&self) -> &Arc<ImageClient> {
        &self.image_client
    }

    /// 获取 token 计数器引用
    pub fn token_counter(&self) -> &Arc<TokenCounter> {
        &self.token_counter
    }

    pub async fn close(&self) {
        info!("[MessageHandler] 关闭资源");
    }

    pub fn extract_user_message(&self, event: &InboundEvent) -> String {
        event
            .message
            .as_ref()
            .map(|m| m.text.clone())
            .unwrap_or_default()
    }

    fn _is_self_message(&self, event: &InboundEvent) -> bool {
        event
            .message
            .as_ref()
            .map(|m| m.sender_name == self.app_config.identity.name)
            .unwrap_or(false)
    }

    fn _is_direct_mention(&self, event: &InboundEvent) -> bool {
        event
            .message
            .as_ref()
            .map(|m| m.is_mention)
            .unwrap_or(false)
    }

    fn _get_sender_name(&self, event: &InboundEvent) -> String {
        event
            .message
            .as_ref()
            .map(|m| m.sender_name.clone())
            .unwrap_or_else(|| "用户".to_string())
    }

    fn _scope_mood_key(&self, event: &InboundEvent) -> String {
        let scope = event
            .message
            .as_ref()
            .map(|m| m.scope.clone())
            .unwrap_or(ChatScope::Private);
        match &scope {
            ChatScope::Private => "private".to_string(),
            ChatScope::Group(gid) => format!("group:{}", gid),
        }
    }

    fn _build_mood_state_block(&self, event: &InboundEvent) -> String {
        let key = self._scope_mood_key(event);
        if let Some(engine) = self.mood_engines.get(&key) {
            if let Some(state) = engine.current() {
                return format!(
                    "当前心情: valence={:.2}, energy={:.2}, arousal={:.2}",
                    state.valence, state.energy, state.arousal
                );
            }
        }
        String::new()
    }

    fn _get_or_create_mood_engine(&self, key: &str) {
        if !self.mood_engines.contains_key(key) {
            self.mood_engines
                .insert(key.to_string(), MoodEngine::new(true));
        }
    }

    /// 判断事件是否包含图片输入
    pub fn has_image_input(&self, event: &InboundEvent) -> bool {
        event
            .attachments
            .iter()
            .any(|a| a.kind.to_lowercase() == "image")
    }

    /// 获取事件中的图片数量
    pub fn get_image_count(&self, event: &InboundEvent) -> usize {
        event
            .attachments
            .iter()
            .filter(|a| a.kind.to_lowercase() == "image")
            .count()
    }

    /// 判断视觉分析是否可用
    pub fn vision_enabled(&self) -> bool {
        self.image_pipeline.is_enabled()
    }

    /// 获取视觉分析状态描述
    pub fn vision_status(&self) -> String {
        if self.image_pipeline.is_enabled() {
            "视觉分析已启用".to_string()
        } else {
            "视觉分析未配置".to_string()
        }
    }

    /// 解析需要 @ 的用户 ID（群聊中被 @ 回复或主动回复时可能需要 @ 用户）
    pub fn resolve_at_user(
        &self,
        event: &InboundEvent,
        plan: Option<&MessageHandlingPlan>,
    ) -> Option<String> {
        let scope = event.message.as_ref().map(|m| m.scope.clone())?;
        if !scope.is_group() {
            return None;
        }
        let reply_context = plan.map(|p| &p.reply_context)?;
        let reply_mode = reply_context
            .get("reply_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();

        if reply_mode == "at" {
            return Some(
                reply_context
                    .get("effective_user_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(
                        &event
                            .message
                            .as_ref()
                            .map(|m| m.sender_id.clone())
                            .unwrap_or_default(),
                    )
                    .to_string(),
            );
        }
        if reply_mode == "proactive" && self.app_config.group_reply.at_user_when_proactive_reply {
            return Some(
                reply_context
                    .get("effective_user_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(
                        &event
                            .message
                            .as_ref()
                            .map(|m| m.sender_id.clone())
                            .unwrap_or_default(),
                    )
                    .to_string(),
            );
        }
        None
    }

    /// 预分析事件中的图片，将分析结果缓存
    pub async fn pre_analyze_images(&self, event: &InboundEvent, _trace_id: &str) {
        if !self.has_image_input(event) || !self.vision_enabled() {
            return;
        }
        let user_message = self.extract_user_message(event);
        let is_group = event
            .message
            .as_ref()
            .map(|m| m.scope.is_group())
            .unwrap_or(false);
        let _ = self
            .image_pipeline
            .analyze(event, &user_message, is_group)
            .await;
    }

    /// 下载事件中的图片，分离普通图片和贴纸，贴纸自动入库
    pub async fn download_images(&self, event: &InboundEvent) -> Vec<String> {
        if !self.has_image_input(event) {
            return Vec::new();
        }
        let user_message = self.extract_user_message(event);
        let is_group = event
            .message
            .as_ref()
            .map(|m| m.scope.is_group())
            .unwrap_or(false);
        match self
            .image_pipeline
            .analyze(event, &user_message, is_group)
            .await
        {
            Ok(analysis) => {
                let descriptions: Vec<String> = analysis
                    .per_image_descriptions
                    .iter()
                    .filter(|s| !s.trim().is_empty())
                    .cloned()
                    .collect();
                descriptions
            }
            Err(_) => Vec::new(),
        }
    }

    /// 分析事件中的图片，返回视觉理解结果
    pub async fn analyze_event_images(
        &self,
        event: &InboundEvent,
        user_text: &str,
        _trace_id: &str,
    ) -> Option<crate::services::vision_client::ImageAnalysisResult> {
        if !self.has_image_input(event) || !self.vision_enabled() {
            return None;
        }
        let is_group = event
            .message
            .as_ref()
            .map(|m| m.scope.is_group())
            .unwrap_or(false);
        self.image_pipeline
            .analyze(event, user_text, is_group)
            .await
            .ok()
    }

    /// 检查并执行速率限制：若未达到发送间隔则等待
    pub async fn get_active_conversation_count(&self) -> usize {
        self.session_manager.count_active().await
    }

    /// 获取助手显示名称
    pub fn get_assistant_name(&self) -> String {
        self.app_config.identity.name.clone()
    }

    /// 获取助手别名
    pub fn get_assistant_alias(&self) -> String {
        self.app_config.identity.alias.clone()
    }

    /// 获取命令帮助文本
    pub fn get_help_text(&self) -> String {
        self.command_handler.get_help_text()
    }

    /// 获取状态文本
    pub fn get_status_text(&self) -> String {
        self.command_handler.get_status_text()
    }

    /// 分割长消息为多个片段
    pub fn split_long_message(&self, message: &str) -> Vec<String> {
        self.text_toolkit.split_long_message(message)
    }

    /// 记录群聊上下文到 ContextRecorder
    pub async fn record_group_context(&self, event: &InboundEvent) {
        let scope = event.message.as_ref().map(|m| m.scope.clone());
        let is_group = scope.as_ref().map(|s| s.is_group()).unwrap_or(false);
        if !is_group {
            return;
        }
        let group_id = scope.and_then(|s| s.group_id().map(|g| g.to_string()));
        if let (Some(gid), Some(msg)) = (group_id, &event.message) {
            let _ = self.context_recorder.get_or_create_log(&gid).await;
            let event_time = msg.timestamp.timestamp_millis() as f64 / 1000.0;
            let _ = self
                .context_recorder
                .record(
                    &gid,
                    &msg.id,
                    &msg.sender_id,
                    &msg.text,
                    event_time,
                    None,
                    None,
                    &msg.sender_name,
                    &msg.text,
                )
                .await;
        }
    }

    /// 收集群聊消息到缓冲区并持久化
    pub async fn collect_group_message(&self, event: &InboundEvent) -> Option<String> {
        self.group_collector.collect_and_persist(event).await
    }

    /// 评估回复效果
    pub async fn evaluate_reply_effect(
        &self,
        event: &InboundEvent,
    ) -> Option<crate::handlers::reply::effect_tracker::ReplyEffectScore> {
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
        let group_id = scope.group_id().unwrap_or("").to_string();
        if group_id.is_empty() {
            return None;
        }
        let user_text = self.extract_user_message(event);
        let message_id = event.message.as_ref().map(|m| m.id.as_str()).unwrap_or("");
        self.reply_side_effects
            .evaluate_reply_effect(&user_id, &group_id, message_id, &user_text, "")
            .await
    }

    /// 应用回复效果评分到角色卡
    pub async fn apply_reply_effect(
        &self,
        event: &InboundEvent,
        score: &crate::handlers::reply::effect_tracker::ReplyEffectScore,
    ) {
        let user_id = event
            .message
            .as_ref()
            .map(|m| m.sender_id.clone())
            .unwrap_or_default();
        if user_id.is_empty() {
            return;
        }
        let scope = event
            .message
            .as_ref()
            .map(|m| m.scope.clone())
            .unwrap_or(ChatScope::Private);
        let group_id = scope.group_id().unwrap_or("").to_string();
        self.reply_side_effects
            .apply_reply_effect(&user_id, &group_id, score)
            .await;
    }

    /// 异步执行协程并设置超时，超时或失败时返回 fallback 值
    pub async fn run_with_timeout<F, T>(
        &self,
        future: F,
        timeout_seconds: f64,
        fallback_value: T,
        label: &str,
    ) -> T
    where
        F: std::future::Future<Output = T>,
    {
        match tokio::time::timeout(Duration::from_secs_f64(timeout_seconds.max(0.05)), future).await
        {
            Ok(result) => result,
            Err(_) => {
                debug!("[消息处理器] {}超时，走降级", label);
                fallback_value
            }
        }
    }
}

/// 截断字符串到指定字符数
fn truncate_str(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}
