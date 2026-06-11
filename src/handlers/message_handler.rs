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
use crate::traits::ai_client::AIClient;
use crate::traits::platform_adapter::PlatformAdapter;
use crate::traits::prompt_template::PromptTemplateLoader;
use crate::traits::timing_gate::{TimingContext, TimingDecision, TimingGateStrategy};

const ACTION_REPLY: &str = "reply";
const ACTION_IGNORE: &str = "ignore";
const ACTION_WAIT: &str = "wait";

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
    conversation_planner: Arc<ConversationPlanner<A>>,
    plan_coordinator: Arc<ConversationPlanCoordinator<A>>,
    context_builder: Arc<ConversationContextBuilder>,
    reply_agent: ReplyAgent<A, L>,
    reply_pipeline: Arc<ReplyPipeline>,

    memory_manager: Arc<MemoryManager>,
    memory_flow_tx: tokio::sync::mpsc::UnboundedSender<MemoryJob>,

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
        memory_manager: Arc<MemoryManager>,
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
        let planner = Arc::new(ConversationPlanner::new(ai_client.clone(), &planner_model));
        let session_mgr = Arc::new(ConversationSessionManager::new(Some(
            memory_manager.conversation_store(),
        )));

        let context_builder = ConversationContextBuilder::new(memory_manager.conversation_store())
            .with_session_manager(session_mgr.clone())
            .with_memory_manager(memory_manager.clone());
        let context_builder = Arc::new(context_builder);

        let plan_coord = ConversationPlanCoordinator::new(
            planner.clone(),
            session_mgr.clone(),
            context_builder.clone(),
            config.identity.name.clone(),
        )
        .with_conversation_store(memory_manager.conversation_store());
        let plan_coord = Arc::new(plan_coord);

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

        let (memory_flow_tx, mut memory_flow_rx) = tokio::sync::mpsc::unbounded_channel();
        {
            let mgr = memory_manager.clone();
            tokio::spawn(async move {
                use crate::memory::memory_dispute_resolver::MemoryDisputeResolver;
                let resolver =
                    MemoryDisputeResolver::new(crate::core::config::MemoryDisputeConfig::default());
                MemoryFlowService::run(mgr, resolver, None, &mut memory_flow_rx).await;
            });
        }

        let db_path = config.memory.db_path.clone();
        let db_dir = std::path::Path::new(&db_path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new("."));
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
        let reply_side_effects = Arc::new(ReplySideEffects::new(reply_effect_tracker.clone()));

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

        let group_collector = Arc::new(
            GroupMessageCollector::new(50)
                .with_conversation_store(memory_manager.conversation_store())
                .with_bot_name(&config.identity.name),
        );

        let context_recorder = Arc::new(ContextRecorder::new(Some(db_path.clone())));

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

        let command_handler = Arc::new(CommandHandler::new(config.clone(), None, None));

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

        info!(
            trace_id = %trace_id,
            execution_key = %execution_key,
            "消息处理开始"
        );

        let plan = self.plan_message(event, &trace_id).await?;

        if !plan.should_reply {
            debug!(trace_id = %trace_id, reason = %plan.reason, "消息处理计划决定不回复");
            return Ok(None);
        }

        let reply_action = self
            .get_ai_response(event, Some(&plan), None, &trace_id)
            .await?;

        if let Some(ref action) = reply_action {
            self.record_reply_sent(event, &action.text, Some(&plan))
                .await;
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

        if scope.is_private() {
            let mut p = empty_plan();
            p.action = ACTION_REPLY.to_string();
            p.reason = "私聊消息，直接回复".to_string();
            p.source = "rule".to_string();
            p.should_reply = true;
            return Ok(p);
        }

        let only_at_mode = self.app_config.group_reply.only_reply_when_at;

        if only_at_mode {
            if self._is_direct_mention(event) {
                let mut p = empty_plan();
                p.action = ACTION_REPLY.to_string();
                p.reason = "群聊仅在被 @ 时回复，当前消息命中 @".to_string();
                p.source = "rule".to_string();
                p.should_reply = true;
                return Ok(p);
            }
            let mut p = empty_plan();
            p.action = ACTION_IGNORE.to_string();
            p.reason = "群聊仅在被 @ 时回复，跳过未 @ 消息".to_string();
            p.source = "rule".to_string();
            return Ok(p);
        }

        if self._is_direct_mention(event) {
            let mut p = empty_plan();
            p.action = ACTION_REPLY.to_string();
            p.reason = "群聊消息显式 @ 了助手，直接回复".to_string();
            p.source = "rule".to_string();
            p.should_reply = true;
            return Ok(p);
        }

        self._plan_with_timing_gate(event, trace_id).await
    }

    async fn _plan_with_timing_gate(
        &self,
        event: &InboundEvent,
        _trace_id: &str,
    ) -> XueliResult<MessageHandlingPlan> {
        let ctx = TimingContext {
            event: event.clone(),
            is_mentioned: self._is_direct_mention(event),
            conversation_active: true,
            time_since_last_reply_secs: 10.0,
            message_count_in_window: 3,
        };

        let decision = self.timing_gate.should_reply(&ctx).await;

        let (action, reason) = match decision {
            Ok(d) => {
                let action = match d {
                    TimingDecision::Reply => ACTION_REPLY,
                    TimingDecision::Wait(_) => ACTION_WAIT,
                    TimingDecision::Ignore => ACTION_IGNORE,
                };
                (action.to_string(), "Time Gate 评估结果".to_string())
            }
            Err(_) => (
                ACTION_IGNORE.to_string(),
                "Time Gate 评估失败，跳过".to_string(),
            ),
        };

        let should_reply = action == ACTION_REPLY;

        Ok(MessageHandlingPlan {
            action,
            reason,
            source: "timing_gate".to_string(),
            should_reply,
            raw_decision: None,
            reply_context: HashMap::new(),
            prompt_plan: None,
            reply_reference: String::new(),
            planning_signals: HashMap::new(),
            planner_caution_hint: None,
            risk_posture: None,
            cached_context: None,
        })
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

        if let Some(cmd_text) = self.command_handler.handle(&user_message) {
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

        if self.image_pipeline.is_enabled() {
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

        let _ = self.memory_flow_tx.send(MemoryJob::RegisterDialogue {
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
        });
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
                let sleep_dur = Duration::from_secs_f64(interval - elapsed.as_secs_f64());
                tokio::time::sleep(sleep_dur).await;
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
            .plan_follow_up(event, &user_message, reply_text, reply_ctx, &trace_id)
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
}
