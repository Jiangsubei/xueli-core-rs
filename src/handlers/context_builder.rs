use std::collections::HashMap;
use std::sync::Arc;

use tracing::debug;

use crate::character::card_service::{CharacterCardService, CharacterCardSnapshot};
use crate::character::narrative::NarrativeService;
use crate::core::drive::engine::DriveEngine;
use crate::core::drive::models::DriveContext;
use crate::core::platform_types::InboundEvent;
use crate::core::scope::ChatScope;
use crate::core::types::{MemoryType, ReplyPlan};
use crate::handlers::image_pipeline::ImagePipeline;
use crate::handlers::reply::style_policy::{FinalStyleGuide, SoftUncertaintySignal};
use crate::handlers::session_manager::ConversationSessionManager;
use crate::memory::internal::access_policy::PromptEntry;
use crate::memory::manager::MemoryManager;
use crate::memory::retrieval::coordinator::{PromptContextResult, RetrievalCoordinator};
use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};
use crate::prelude::{AIClient, XueliResult};
use crate::traits::prompt_template::PromptTemplateLoader;

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
    /// 长期记忆上下文（持久记忆）
    pub persistent_memory_context: Option<String>,
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
    /// 角色卡快照（当前用户）
    pub character_card_snapshot: Option<CharacterCardSnapshot>,
    /// 叙事线摘要
    pub narrative_thread_summary: Option<String>,
    /// 叙事线标签
    pub narrative_thread_label: Option<String>,
    /// 长期相处脉络（叙事自我）
    pub narrative_self: Option<HashMap<String, String>>,
    /// 谨慎度信号
    pub caution_signal: Option<HashMap<String, String>>,
    /// 规划信号
    pub planning_signals: Option<HashMap<String, String>>,
    /// 用户情绪标签
    pub user_emotion_label: Option<String>,
    /// 风格指引（最终）
    pub style_guide: Option<FinalStyleGuide>,
    /// 软不确定性信号
    pub soft_uncertainty_signals: Option<Vec<SoftUncertaintySignal>>,
    /// 内驱力上下文（情绪/动机/关系状态）
    pub drive_context: Option<DriveContext>,
    /// 谨慎事件检测结果（{event_type: description}）
    pub caution_events: Option<Vec<(String, String)>>,
    /// 用户画像信号（来自 reply_adaptation_signal）
    pub user_profile_signal: Option<HashMap<String, serde_json::Value>>,
    /// 风格适配信号（从 user_profile_signal 解析）
    pub style_adaptation_signal: Option<HashMap<String, serde_json::Value>>,
    /// 关系状态信号（从 user_profile_signal 解析）
    pub relationship_state_signal: Option<HashMap<String, serde_json::Value>>,
}

/// 会话上下文构建器 — 从存储和会话管理器加载历史，构建完整上下文。
///
/// 集成 MemoryManager、CharacterCardService、NarrativeService、
/// ReplyStylePolicy 等组件，对应 Python 版 `ConversationContextBuilder`。
pub struct ConversationContextBuilder<
    L: PromptTemplateLoader + 'static = crate::services::prompt_loader::NoopPromptTemplateLoader,
    A: AIClient + 'static = crate::services::ai_client::NoopAIClient,
> {
    store: Arc<SqliteConversationStore>,
    session_manager: Option<Arc<ConversationSessionManager>>,
    memory_manager: Option<Arc<MemoryManager<L>>>,
    retrieval_coordinator: Option<Arc<RetrievalCoordinator>>,
    character_card_service: Option<Arc<CharacterCardService>>,
    narrative_service: Option<Arc<NarrativeService>>,
    drive_engine: Option<Arc<DriveEngine>>,
    image_pipeline: Option<Arc<ImagePipeline<A, L>>>,
    style_policy: Option<crate::handlers::reply::style_policy::ReplyStylePolicy<L>>,
}

impl<L: PromptTemplateLoader + 'static, A: AIClient + 'static> ConversationContextBuilder<L, A> {
    pub fn new(store: Arc<SqliteConversationStore>) -> Self {
        Self {
            store,
            session_manager: None,
            memory_manager: None,
            retrieval_coordinator: None,
            character_card_service: None,
            narrative_service: None,
            drive_engine: None,
            image_pipeline: None,
            style_policy: None,
        }
    }

    /// 设置会话管理器（用于内存会话追踪）
    pub fn with_session_manager(mut self, mgr: Arc<ConversationSessionManager>) -> Self {
        self.session_manager = Some(mgr);
        self
    }

    /// 设置记忆管理器（用于记忆上下文加载）
    pub fn with_memory_manager(mut self, mgr: Arc<MemoryManager<L>>) -> Self {
        self.memory_manager = Some(mgr);
        self
    }

    /// 设置统一检索协调器（用于多记忆层上下文组装）
    pub fn with_retrieval_coordinator(mut self, coord: Arc<RetrievalCoordinator>) -> Self {
        self.retrieval_coordinator = Some(coord);
        self
    }

    /// 设置角色卡服务（用于加载角色快照）
    pub fn with_character_card_service(mut self, svc: Arc<CharacterCardService>) -> Self {
        self.character_card_service = Some(svc);
        self
    }

    /// 设置叙事服务（用于加载叙事线）
    pub fn with_narrative_service(mut self, svc: Arc<NarrativeService>) -> Self {
        self.narrative_service = Some(svc);
        self
    }

    /// 设置内驱力引擎（用于加载驱动力上下文）
    pub fn with_drive_engine(mut self, engine: Arc<DriveEngine>) -> Self {
        self.drive_engine = Some(engine);
        self
    }

    /// 设置图片管线（用于视觉上下文分析）
    pub fn with_image_pipeline(mut self, pipeline: Arc<ImagePipeline<A, L>>) -> Self {
        self.image_pipeline = Some(pipeline);
        self
    }

    /// 设置回复风格策略（用于构建最终风格指引）
    pub fn with_style_policy(
        mut self,
        policy: crate::handlers::reply::style_policy::ReplyStylePolicy<L>,
    ) -> Self {
        self.style_policy = Some(policy);
        self
    }

    /// 从事件和回复计划构建上下文
    pub async fn build(
        &self,
        event: &InboundEvent,
        _plan: &ReplyPlan,
    ) -> XueliResult<ConversationContext> {
        self.build_with_signals(event, _plan, None).await
    }

    /// 从事件、回复计划及上游规划信号构建上下文
    pub async fn build_with_signals(
        &self,
        event: &InboundEvent,
        _plan: &ReplyPlan,
        external_planning_signals: Option<&HashMap<String, serde_json::Value>>,
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

        let stored_records = self
            .store
            .get_recent_by_scope(scope_type, scope_id, 20)
            .await?;

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
                let follows = msgs.last().map(|m| m.role == "assistant").unwrap_or(false);
                let cont = if msgs.len() >= 2 {
                    Some("soft_continuation".to_string())
                } else {
                    Some("unknown".to_string())
                };
                (restore_text, follows, cont)
            } else {
                (None, false, None)
            };

        // 构建用户情绪标签（提前计算，供记忆检索使用）
        let mut user_emotion_label = self.build_user_emotion_label(&user_id);

        // 若上游 TimingGate/Planner 提供了情绪标签，优先使用
        if let Some(signals) = external_planning_signals {
            if let Some(label) = signals
                .get("user_emotion_label")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
            {
                if !label.is_empty() {
                    user_emotion_label = Some(label);
                }
            }
        }

        // 加载记忆上下文（通过 RetrievalCoordinator 统一检索）
        let (person_facts, persistent_memory, precise_recall, dynamic_memory) = self
            .load_memory_context(
                &user_id,
                &user_message,
                &scope,
                user_emotion_label.as_deref(),
            )
            .await;

        // 若统一检索未产生动态记忆，回退到会话摘要
        let dynamic_memory = dynamic_memory.or_else(|| {
            if !stored_records.is_empty() {
                Some(format!(
                    "近期对话总计 {} 条，当前为{}聊。",
                    stored_records.len(),
                    scope_type
                ))
            } else {
                None
            }
        });

        // 加载视觉上下文（通过 ImagePipeline 实际分析）
        let vision_description = self.load_vision_context(event).await;

        // 加载角色卡快照
        let character_card_snapshot = self.load_character_card_snapshot(&user_id);

        // 加载叙事线（使用 get_thread_summary 统一入口）
        let (mut narrative_thread_summary, mut narrative_thread_label, narrative_self) =
            self.load_narrative_thread(&user_id);

        // 解析上游叙事信号，覆盖叙事线摘要/标签
        let narrative_signal = external_planning_signals
            .and_then(|s| s.get("narrative_signal"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(label) = narrative_signal
            .get("narrative_label")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
        {
            if !label.is_empty() {
                narrative_thread_label = Some(label);
            }
        }
        if let Some(summary) = narrative_signal
            .get("narrative_summary")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
        {
            if !summary.is_empty() {
                narrative_thread_summary = Some(summary);
            }
        }

        // 解析上游回复适配信号
        let reply_adaptation_signal = external_planning_signals
            .and_then(|s| {
                s.get("reply_adaptation_signal")
                    .or_else(|| s.get("user_profile_signal"))
            })
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let user_profile_signal = if reply_adaptation_signal.is_object() {
            Some(
                reply_adaptation_signal
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect::<HashMap<String, serde_json::Value>>(),
            )
        } else {
            None
        };
        let style_adaptation_signal = user_profile_signal.as_ref().and_then(|up| {
            let mut map = HashMap::new();
            for key in &[
                "style_summary",
                "preferred_response_shape",
                "followup_preference",
                "directness",
                "humor_tolerance",
                "confidence",
                "reason",
            ] {
                if let Some(v) = up.get(*key) {
                    map.insert(key.to_string(), v.clone());
                }
            }
            if map.is_empty() {
                None
            } else {
                Some(map)
            }
        });
        let relationship_state_signal = user_profile_signal.as_ref().and_then(|up| {
            let mut map = HashMap::new();
            for key in &[
                "relationship_read",
                "trust_level",
                "formality_distance",
                "emotional_safety",
                "tone_guidance",
                "last_emotional_tone",
                "confidence",
                "reason",
            ] {
                if let Some(v) = up.get(*key) {
                    map.insert(key.to_string(), v.clone());
                }
            }
            if map.is_empty() {
                None
            } else {
                Some(map)
            }
        });

        // 加载内驱力上下文
        let drive_context = self.load_drive_context(&user_id);

        // 加载软不确定性信号
        let soft_uncertainty_signals = self.load_soft_uncertainty_signals(&user_id).await;

        // 检测谨慎事件
        let caution_events = self.detect_caution_events(&user_message, &recent_messages);

        // 构建谨慎度信号
        let caution_signal = self.build_caution_signal(
            &person_facts,
            &persistent_memory,
            &precise_recall,
            &dynamic_memory,
            soft_uncertainty_signals.as_ref().map(|s| s.as_slice()),
        );

        // 规划信号归一化
        let planning_signals = self.normalize_planning_signals(_plan);

        // 构建最终风格指引
        let style_guide = self
            .build_style_guide(scope_type, _plan, soft_uncertainty_signals.as_deref())
            .await;

        Ok(ConversationContext {
            user_message,
            recent_messages,
            conversation_key,
            user_id,
            scope,
            is_group,
            is_first_turn,
            person_facts,
            persistent_memory_context: persistent_memory,
            dynamic_memory,
            session_restore,
            precise_recall,
            vision_description,
            continuity_hint: continuity,
            follows_assistant_recently: follows_assistant,
            recent_message_count,
            character_card_snapshot,
            narrative_thread_summary,
            narrative_thread_label,
            narrative_self,
            caution_signal,
            planning_signals,
            user_emotion_label,
            style_guide,
            soft_uncertainty_signals,
            drive_context,
            caution_events,
            user_profile_signal,
            style_adaptation_signal,
            relationship_state_signal,
        })
    }

    /// 构建最终风格指引
    async fn build_style_guide(
        &self,
        scope_type: &str,
        plan: &ReplyPlan,
        uncertainty_signals: Option<&[SoftUncertaintySignal]>,
    ) -> Option<FinalStyleGuide> {
        let policy = self.style_policy.as_ref()?;

        let chat_mode = if scope_type == "group" {
            "group"
        } else {
            "private"
        };
        let tone_profile = plan.style.as_deref().unwrap_or("balanced");
        let initiative = "gentle_follow";
        let expression_profile = "plain";
        let reply_goal = "continue";

        Some(
            policy
                .build(
                    chat_mode,
                    "",
                    tone_profile,
                    initiative,
                    expression_profile,
                    reply_goal,
                    None,
                    uncertainty_signals,
                )
                .await,
        )
    }

    /// 通过 RetrievalCoordinator 统一检索加载多记忆层上下文
    ///
    /// 返回：(人物事实, 持久记忆, 精确回忆, 动态记忆)
    async fn load_memory_context(
        &self,
        user_id: &str,
        query: &str,
        scope: &ChatScope,
        user_emotion_label: Option<&str>,
    ) -> (
        Option<Vec<String>>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        // 无查询文本时回退到简单用户记忆过滤（避免无意义检索）
        if query.trim().is_empty() {
            debug!("查询文本为空，回退到按用户加载记忆");
            return self.load_memory_context_fallback(user_id).await;
        }

        let rc = match &self.retrieval_coordinator {
            Some(c) => c,
            None => {
                debug!("跳过统一记忆检索：retrieval_coordinator 未注入");
                return self.load_memory_context_fallback(user_id).await;
            }
        };

        let emotion = user_emotion_label.unwrap_or("");
        let result = match rc
            .search_memories_with_context(user_id, query, scope, true, emotion)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                debug!("统一记忆检索失败: {}", e);
                return self.load_memory_context_fallback(user_id).await;
            }
        };

        // 若检索结果完全为空，回退到按用户过滤
        let has_any = result.sections.values().any(|v| !v.is_empty())
            || !result.precise_recall.is_empty()
            || !result.dynamic_memories.is_empty();
        if !has_any {
            return self.load_memory_context_fallback(user_id).await;
        }

        let person_facts = Self::extract_person_facts(&result);
        let persistent_memory = Self::format_persistent_memory(&result);
        let precise_recall = Self::format_precise_recall(&result);
        let dynamic_memory = Self::format_dynamic_memory(&result);

        (
            person_facts,
            persistent_memory,
            precise_recall,
            dynamic_memory,
        )
    }

    /// 按用户简单过滤记忆的回退实现
    async fn load_memory_context_fallback(
        &self,
        user_id: &str,
    ) -> (
        Option<Vec<String>>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        let mm = match &self.memory_manager {
            Some(m) => m,
            None => {
                debug!("跳过记忆上下文加载：memory_manager 未注入");
                return (None, None, None, None);
            }
        };

        // 人物事实
        let person_facts: Option<Vec<String>> = match mm.get_by_user(user_id).await {
            Ok(items) => {
                let facts: Vec<String> = items
                    .iter()
                    .filter(|item| {
                        matches!(
                            item.memory_type,
                            MemoryType::Fact | MemoryType::Preference | MemoryType::Relationship
                        )
                    })
                    .take(6)
                    .map(|item| item.content.clone())
                    .collect();
                if facts.is_empty() {
                    None
                } else {
                    Some(facts)
                }
            }
            Err(_) => None,
        };

        // 持久记忆（按重要度排序）
        let persistent_memory: Option<String> = match mm.get_by_user(user_id).await {
            Ok(items) => {
                let mut important: Vec<_> = items
                    .iter()
                    .filter(|item| item.importance > 0.5)
                    .cloned()
                    .collect();
                important.sort_by(|a, b| {
                    b.importance
                        .partial_cmp(&a.importance)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let lines: Vec<String> = important
                    .iter()
                    .take(5)
                    .map(|item| item.content.clone())
                    .collect();
                if lines.is_empty() {
                    None
                } else {
                    Some(crate::handlers::reply::pipeline::format_memory_context(
                        &lines,
                    ))
                }
            }
            Err(_) => None,
        };

        (person_facts, persistent_memory, None, None)
    }

    fn extract_person_facts(result: &PromptContextResult) -> Option<Vec<String>> {
        let mut facts: Vec<String> = Vec::new();
        for key in &["user_important", "addressing", "shared"] {
            if let Some(entries) = result.sections.get(*key) {
                for entry in entries {
                    if let Some(content) = entry.get("content").and_then(|v| v.as_str()) {
                        let text = content.trim();
                        if !text.is_empty() && !facts.contains(&text.to_string()) {
                            facts.push(text.to_string());
                        }
                    }
                }
            }
        }
        if facts.is_empty() {
            None
        } else {
            Some(facts)
        }
    }

    fn format_persistent_memory(result: &PromptContextResult) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(entries) = result.sections.get("user_important") {
            if !entries.is_empty() {
                parts.push("=== 当前用户重要记忆 ===".to_string());
                parts.push(Self::format_entries(entries));
            }
        }
        if let Some(entries) = result.sections.get("addressing") {
            if !entries.is_empty() {
                parts.push("=== 当前场景称呼要求 ===".to_string());
                parts.push(Self::format_entries(entries));
            }
        }
        if let Some(entries) = result.sections.get("shared") {
            if !entries.is_empty() {
                parts.push("=== 当前场景共享规则 / 共享重要记忆 ===".to_string());
                parts.push(Self::format_entries(entries));
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }

    fn format_precise_recall(result: &PromptContextResult) -> Option<String> {
        if result.precise_recall.is_empty() {
            return None;
        }
        Some(Self::format_entries(&result.precise_recall))
    }

    fn format_dynamic_memory(result: &PromptContextResult) -> Option<String> {
        if result.dynamic_memories.is_empty() {
            return None;
        }
        Some(Self::format_entries(&result.dynamic_memories))
    }

    fn format_entries(entries: &[PromptEntry]) -> String {
        entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let content = entry
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                format!("{}. {}", idx + 1, content)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 通过 CharacterCardService 加载角色卡快照
    fn load_character_card_snapshot(&self, user_id: &str) -> Option<CharacterCardSnapshot> {
        let svc = self.character_card_service.as_ref()?;
        let snap = svc.get_snapshot(user_id);
        // 仅当有实际内容时返回
        if snap.core_traits.is_empty()
            && snap.tone_preferences.is_empty()
            && snap.bot_persona_hints.is_empty()
            && snap.emotional_trend.is_empty()
        {
            None
        } else {
            Some(snap)
        }
    }

    /// 通过 NarrativeService 加载叙事线程
    fn load_narrative_thread(
        &self,
        user_id: &str,
    ) -> (
        Option<String>,
        Option<String>,
        Option<HashMap<String, String>>,
    ) {
        let svc = match &self.narrative_service {
            Some(s) => s,
            None => {
                debug!("跳过叙事线程加载：narrative_service 未注入");
                return (None, None, None);
            }
        };

        let thread = svc.get_thread(user_id);
        let summary = if thread.summary.is_empty() {
            None
        } else {
            Some(thread.summary.clone())
        };
        let label = if thread.theme.is_empty() || thread.theme == "default" {
            None
        } else {
            Some(thread.theme.clone())
        };

        // 构建叙事自我（narrative_self）
        let narrative_self: Option<HashMap<String, String>> = {
            let story: String = thread
                .events
                .iter()
                .rev()
                .take(5)
                .map(|e| e.description.clone())
                .collect::<Vec<_>>()
                .join("\n");
            if story.is_empty() {
                None
            } else {
                let mut map = HashMap::new();
                map.insert("relationship_story".to_string(), story);
                map.insert(
                    "turn_count".to_string(),
                    thread.turn_count_since_last_update.to_string(),
                );
                Some(map)
            }
        };

        (summary, label, narrative_self)
    }

    /// 构建谨慎度信号 — 基于记忆可用性、不确定性等条件
    ///
    /// 返回 {caution_level, caution_reasons, reply_guidance} 的映射
    fn build_caution_signal(
        &self,
        person_facts: &Option<Vec<String>>,
        persistent_memory: &Option<String>,
        precise_recall: &Option<String>,
        dynamic_memory: &Option<String>,
        soft_uncertainty_signals: Option<&[SoftUncertaintySignal]>,
    ) -> Option<HashMap<String, String>> {
        let mut reasons: Vec<String> = Vec::new();
        let mut guidance: Vec<String> = Vec::new();

        // 检查记忆上下文是否全部为空
        let has_person_facts = person_facts
            .as_ref()
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        let has_persistent = persistent_memory
            .as_ref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let has_precise = precise_recall
            .as_ref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let has_dynamic = dynamic_memory
            .as_ref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);

        let memory_available = has_person_facts || has_persistent || has_precise || has_dynamic;

        if !memory_available {
            reasons.push("memory_context_empty".to_string());
            guidance.push("没有可靠记忆依据时，避免假装记得细节。".to_string());
        }

        // 软不确定性信号：证据不足的事实
        if let Some(signals) = soft_uncertainty_signals {
            if !signals.is_empty() {
                reasons.push("soft_uncertainty".to_string());
                guidance.push("部分事实证据不足，回复时避免断言不明确的信息。".to_string());
            }
        }

        // 无原因则返回低谨慎度
        let unique_reasons: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            reasons
                .into_iter()
                .filter(|r| seen.insert(r.clone()))
                .collect()
        };

        if unique_reasons.is_empty() {
            return None;
        }

        let high_reasons: std::collections::HashSet<&str> = [
            "soft_uncertainty",
            "low_emotional_safety",
            "high_interruption_risk",
            "negative_feedback_history",
        ]
        .iter()
        .cloned()
        .collect();

        let level = if unique_reasons
            .iter()
            .any(|r| high_reasons.contains(r.as_str()))
        {
            "high"
        } else {
            "medium"
        };

        let mut map = HashMap::new();
        map.insert("caution_level".to_string(), level.to_string());
        map.insert("caution_reasons".to_string(), unique_reasons.join(","));
        let deduped_guidance: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            guidance
                .into_iter()
                .filter(|g| seen.insert(g.clone()))
                .collect()
        };
        map.insert("reply_guidance".to_string(), deduped_guidance.join("；"));

        Some(map)
    }

    /// 加载视觉上下文 — 通过 ImagePipeline 分析图片并生成描述
    ///
    /// 对应 Python 版 `_load_vision_context()`
    async fn load_vision_context(&self, event: &InboundEvent) -> Option<String> {
        let pipeline = match &self.image_pipeline {
            Some(p) => p,
            None => {
                debug!("跳过视觉上下文加载：image_pipeline 未注入");
                return Self::placeholder_vision_context(event);
            }
        };

        if !pipeline.is_enabled() {
            return Self::placeholder_vision_context(event);
        }

        if !Self::event_has_image(event) {
            return None;
        }

        let user_message = event
            .message
            .as_ref()
            .map(|m| m.text.clone())
            .unwrap_or_default();
        let is_group = event
            .message
            .as_ref()
            .map(|m| m.scope.is_group())
            .unwrap_or(false);

        match pipeline.analyze(event, &user_message, is_group).await {
            Ok(analysis) => {
                if analysis.has_usable_description() {
                    Some(format!("[图片] {}", analysis.merged_description))
                } else {
                    Some(format!(
                        "[图片] 分析未返回可用描述（成功{}，失败{}）",
                        analysis.success_count, analysis.failure_count
                    ))
                }
            }
            Err(e) => {
                debug!("视觉分析失败: {}", e);
                Some(format!("[图片] 分析失败: {}", e))
            }
        }
    }

    fn event_has_image(event: &InboundEvent) -> bool {
        event
            .attachments
            .iter()
            .any(|a| a.kind.to_lowercase() == "image")
    }

    fn placeholder_vision_context(event: &InboundEvent) -> Option<String> {
        let count = event
            .attachments
            .iter()
            .filter(|a| a.kind.to_lowercase() == "image")
            .count();
        if count == 0 {
            None
        } else if count == 1 {
            Some("[图片]（视觉分析待处理）".to_string())
        } else {
            Some(format!("[图片] {}张图片（视觉分析待处理）", count))
        }
    }

    /// 构建用户情绪标签 — 从角色卡快照中提取
    ///
    /// 对应 Python 版 `_build_user_emotion_label()`
    fn build_user_emotion_label(&self, user_id: &str) -> Option<String> {
        let ccs = self.character_card_service.as_ref()?;
        let snapshot = ccs.get_snapshot(user_id);

        if snapshot.emotional_trend.is_empty() {
            None
        } else {
            Some(snapshot.emotional_trend.clone())
        }
    }

    // ─── Phase 2.2 增强 ──────────────────────────────────

    /// 加载内驱力上下文 — 从 DriveEngine 获取情绪/动机/关系状态
    fn load_drive_context(&self, user_id: &str) -> Option<DriveContext> {
        let engine = match &self.drive_engine {
            Some(e) => e,
            None => {
                debug!("跳过内驱力上下文加载：drive_engine 未注入");
                return None;
            }
        };
        if !engine.enabled() {
            debug!("跳过内驱力上下文加载：DriveEngine 未启用");
            return None;
        }
        Some(engine.get_drive_context(user_id))
    }

    /// 加载软不确定性信号 — 按事实查询证据数量，识别证据不足的事实
    async fn load_soft_uncertainty_signals(
        &self,
        user_id: &str,
    ) -> Option<Vec<SoftUncertaintySignal>> {
        let mm = match &self.memory_manager {
            Some(m) => m,
            None => {
                debug!("跳过软不确定性信号加载：memory_manager 未注入");
                return None;
            }
        };

        let memories = match mm.get_by_user(user_id).await {
            Ok(items) => items,
            Err(e) => {
                debug!("加载用户记忆失败: {}", e);
                return None;
            }
        };
        if memories.is_empty() {
            return None;
        }

        let store = mm.fact_evidence_store();
        let mut signals: Vec<SoftUncertaintySignal> = Vec::new();

        for memory in memories.iter().filter(|m| {
            matches!(
                m.memory_type,
                MemoryType::Fact | MemoryType::Preference | MemoryType::Relationship
            )
        }) {
            let evidence_count = match store.get_by_fact(&memory.id).await {
                Ok(ev) => ev.len(),
                Err(e) => {
                    debug!("查询事实证据失败: {}", e);
                    0
                }
            };
            if evidence_count < 2 {
                signals.push(SoftUncertaintySignal {
                    reason: format!(
                        "事实'{}'的证据来源有限（仅{}条证据）",
                        memory.id, evidence_count
                    ),
                });
            }
        }

        if signals.is_empty() {
            None
        } else {
            Some(signals)
        }
    }

    /// 检测所有谨慎事件，返回检测到的事件列表
    fn detect_caution_events(
        &self,
        message: &str,
        recent_messages: &[String],
    ) -> Option<Vec<(String, String)>> {
        let text = message.trim();
        if text.is_empty() {
            return None;
        }

        let mut events: Vec<(String, String)> = Vec::new();

        if let Some(desc) = Self::detect_repeated_question(text, recent_messages) {
            events.push(("repeated_question".to_string(), desc));
        }
        if let Some(desc) = Self::detect_sensitive_topic(text) {
            events.push(("sensitive_topic".to_string(), desc));
        }
        if let Some(desc) = Self::detect_new_topic(text, recent_messages) {
            events.push(("new_topic".to_string(), desc));
        }
        if let Some(desc) = Self::detect_user_contradiction(text, recent_messages) {
            events.push(("user_contradiction".to_string(), desc));
        }
        if let Some(desc) = Self::detect_high_emotion(text) {
            events.push(("high_emotion".to_string(), desc));
        }
        if let Some(desc) = Self::detect_cross_questioning(text) {
            events.push(("cross_questioning".to_string(), desc));
        }
        if let Some(desc) = Self::detect_incoherence(text, recent_messages) {
            events.push(("incoherence".to_string(), desc));
        }
        if let Some(desc) = Self::detect_low_credibility(text) {
            events.push(("low_credibility".to_string(), desc));
        }
        if let Some(desc) = Self::detect_stale_topic(text, recent_messages) {
            events.push(("stale_topic".to_string(), desc));
        }

        if events.is_empty() {
            None
        } else {
            Some(events)
        }
    }

    /// 检测重复提问：用户短时间内重复相同或高度相似的问题
    fn detect_repeated_question(text: &str, recent_messages: &[String]) -> Option<String> {
        if recent_messages.is_empty() {
            return None;
        }
        let normalized = text.trim().to_lowercase();
        // 仅检查长度 >= 4 的文本，避免短回复误判
        if normalized.chars().count() < 4 {
            return None;
        }
        let mut count = 0usize;
        for msg in recent_messages {
            let lower = msg.trim().to_lowercase();
            // 简单相似度：完全相同或包含关系
            if lower == normalized || lower.contains(&normalized) || normalized.contains(&lower) {
                count += 1;
            }
        }
        if count >= 2 {
            Some(format!(
                "用户重复提问（近{}条消息中出现{}次相似内容）",
                recent_messages.len(),
                count
            ))
        } else {
            None
        }
    }

    /// 检测敏感话题：涉及政治、暴力、违法等敏感内容
    fn detect_sensitive_topic(text: &str) -> Option<String> {
        let keywords = [
            "自杀", "自残", "暴力", "违法", "毒品", "赌博", "传销", "诈骗", "色情", "赌博",
        ];
        let lower = text.to_lowercase();
        for kw in &keywords {
            if lower.contains(kw) {
                return Some(format!("检测到敏感话题关键词：{}", kw));
            }
        }
        None
    }

    /// 检测新话题引入：消息与近期对话主题明显不同
    fn detect_new_topic(text: &str, recent_messages: &[String]) -> Option<String> {
        if recent_messages.len() < 3 {
            return None;
        }
        let normalized = text.trim().to_lowercase();
        if normalized.chars().count() < 6 {
            return None;
        }
        // 检查是否与近期消息有重合词汇
        let recent_text = recent_messages.join(" ").to_lowercase();
        let words: Vec<&str> = normalized
            .split(|c: char| !c.is_alphanumeric() && !c.is_whitespace())
            .filter(|w| w.chars().count() >= 2)
            .collect();
        let overlap_count = words.iter().filter(|w| recent_text.contains(**w)).count();
        let overlap_ratio = if words.is_empty() {
            1.0
        } else {
            overlap_count as f64 / words.len() as f64
        };
        if overlap_ratio < 0.2 {
            Some(format!(
                "用户引入新话题，与近期对话词汇重合率仅{:.0}%",
                overlap_ratio * 100.0
            ))
        } else {
            None
        }
    }

    /// 检测用户矛盾：用户当前消息与近期之前的表态矛盾
    fn detect_user_contradiction(text: &str, recent_messages: &[String]) -> Option<String> {
        if recent_messages.len() < 2 {
            return None;
        }
        let normalized = text.trim().to_lowercase();
        if normalized.chars().count() < 10 {
            return None;
        }
        let negation_markers = [
            "不是", "不对", "错了", "不再", "改了", "变了", "之前", "以前", "原来", "其实",
        ];
        let has_negation = negation_markers.iter().any(|m| normalized.contains(m));
        if !has_negation {
            return None;
        }
        // 检查最近的用户消息是否有矛盾信号
        // 取倒数第2条（最近的是 assistant，倒数第2条是用户上一条）
        let mut prev_user_msgs: Vec<&str> = recent_messages
            .iter()
            .rev()
            .filter(|m| {
                // 用户消息通常以 "用户" 或 sender 名开头
                m.contains("用户") || m.contains("user")
            })
            .take(2)
            .map(|s| s.as_str())
            .collect();
        if prev_user_msgs.len() < 2 {
            return None;
        }
        prev_user_msgs.reverse(); // 恢复时间顺序
                                  // 简单检测：如果前一条用户消息不含否定词，当前含否定词，可能矛盾
        let prev_has_negation = negation_markers
            .iter()
            .any(|m| prev_user_msgs[0].to_lowercase().contains(m));
        if !prev_has_negation {
            Some("用户当前消息含否定/修正表述，可能与此前表态矛盾".to_string())
        } else {
            None
        }
    }

    /// 检测高情绪：用户消息中出现强烈情绪表达
    fn detect_high_emotion(text: &str) -> Option<String> {
        let high_emotion_markers = [
            "!!",
            "！！！",
            "？？？",
            "啊啊啊",
            "呜呜",
            "气死",
            "烦死",
            "崩溃",
            "太棒了",
            "太开心",
            "好激动",
            "受不了",
            "救命",
            "卧槽",
            "我靠",
            "天哪",
            "我的天",
        ];
        let lower = text.to_lowercase();
        let mut found: Vec<&str> = Vec::new();
        for marker in &high_emotion_markers {
            if lower.contains(marker) {
                found.push(marker);
            }
        }
        // 连续感叹号/问号
        let exclamation_count = text.chars().filter(|c| *c == '!' || *c == '！').count();
        let question_count = text.chars().filter(|c| *c == '?' || *c == '？').count();
        if exclamation_count >= 3 {
            found.push("连续感叹号");
        }
        if question_count >= 3 {
            found.push("连续问号");
        }

        if found.is_empty() {
            None
        } else {
            Some(format!("用户消息情绪强烈：{}", found.join("、")))
        }
    }

    /// 检测交叉提问：用户在同一消息中提出多个不相关的问题
    fn detect_cross_questioning(text: &str) -> Option<String> {
        let question_count = text
            .chars()
            .filter(|c| *c == '?' || *c == '？' || *c == '?')
            .count();
        // 同时检测多个疑问句标记
        let question_markers = [
            "什么",
            "怎么",
            "为什么",
            "如何",
            "哪里",
            "谁",
            "几点",
            "多少",
        ];
        let marker_count = question_markers
            .iter()
            .filter(|m| text.contains(**m))
            .count();
        // 至少2个问号或2个疑问标记
        if question_count >= 2 || marker_count >= 2 {
            Some(format!(
                "用户交叉提问（{}个问号，{}个疑问标记）",
                question_count, marker_count
            ))
        } else {
            None
        }
    }

    /// 检测不连贯：用户消息与近期对话缺乏逻辑关联
    fn detect_incoherence(text: &str, recent_messages: &[String]) -> Option<String> {
        if recent_messages.len() < 2 {
            return None;
        }
        let normalized = text.trim().to_lowercase();
        if normalized.chars().count() < 6 {
            return None;
        }
        // 取最近1条消息，检查是否有共享词汇
        let last_msg = recent_messages.last()?.to_lowercase();
        let words: Vec<&str> = normalized
            .split(|c: char| !c.is_alphanumeric() && !c.is_whitespace())
            .filter(|w| w.chars().count() >= 2)
            .collect();
        let has_overlap = words.iter().any(|w| last_msg.contains(*w));
        if !has_overlap && words.len() >= 3 {
            Some("用户消息与上一条对话无共享词汇，可能不连贯".to_string())
        } else {
            None
        }
    }

    /// 检测低可信：用户消息包含不确定/猜测性表述
    fn detect_low_credibility(text: &str) -> Option<String> {
        let uncertainty_markers = [
            "可能",
            "也许",
            "大概",
            "好像",
            "似乎",
            "听说",
            "据说",
            "不确定",
            "不清楚",
            "不知道是不是",
            "感觉",
            "应该是",
            "或许是",
            "或许是",
            "说不定",
        ];
        let lower = text.to_lowercase();
        let count = uncertainty_markers
            .iter()
            .filter(|m| lower.contains(**m))
            .count();
        if count >= 2 {
            Some(format!("用户消息包含{}个不确定表述，可信度较低", count))
        } else {
            None
        }
    }

    /// 检测陈旧话题：用户重新提及很久以前的话题
    fn detect_stale_topic(text: &str, recent_messages: &[String]) -> Option<String> {
        if recent_messages.len() < 5 {
            return None;
        }
        let normalized = text.trim().to_lowercase();
        if normalized.chars().count() < 6 {
            return None;
        }
        // 检查消息是否与较旧的消息（前一半）有关联，而与最近的消息（后一半）无关联
        let mid = recent_messages.len() / 2;
        let old_messages = &recent_messages[..mid];
        let recent_half = &recent_messages[mid..];

        let old_text = old_messages.join(" ").to_lowercase();
        let recent_text = recent_half.join(" ").to_lowercase();

        let words: Vec<&str> = normalized
            .split(|c: char| !c.is_alphanumeric() && !c.is_whitespace())
            .filter(|w| w.chars().count() >= 2)
            .collect();
        let old_overlap = words.iter().filter(|w| old_text.contains(**w)).count();
        let recent_overlap = words.iter().filter(|w| recent_text.contains(**w)).count();

        if old_overlap >= 2 && recent_overlap == 0 && !words.is_empty() {
            Some(format!(
                "用户重新提及较旧话题，与近期{}条消息无关",
                recent_half.len()
            ))
        } else {
            None
        }
    }

    /// 规划信号归一化 — 将 ReplyPlan 标准化为键值对映射
    fn normalize_planning_signals(&self, plan: &ReplyPlan) -> Option<HashMap<String, String>> {
        let mut map = HashMap::new();

        if let Some(ref topic) = plan.topic {
            map.insert("topic".to_string(), topic.clone());
        }
        if let Some(ref style) = plan.style {
            map.insert("style".to_string(), style.clone());
        }
        map.insert(
            "memory_recall_needed".to_string(),
            plan.memory_recall_needed.to_string(),
        );
        map.insert("use_emoji".to_string(), plan.use_emoji.to_string());
        map.insert("priority".to_string(), plan.priority.to_string());

        if map.is_empty() {
            None
        } else {
            Some(map)
        }
    }
}

impl Default
    for ConversationContextBuilder<
        crate::services::prompt_loader::FilePromptTemplateLoader,
        crate::services::ai_client::NoopAIClient,
    >
{
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
