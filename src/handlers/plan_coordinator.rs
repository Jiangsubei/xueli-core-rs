use std::collections::HashMap;
use std::sync::Arc;

use crate::character::card_service::{CharacterCardService, CharacterCardSnapshot};
use crate::core::platform_types::InboundEvent;
use crate::core::scope::ChatScope;
use crate::core::types::{MessageHandlingPlan, PromptPlan, ReplyPlan};
use crate::handlers::context_builder::{ConversationContext, ConversationContextBuilder};
use crate::handlers::planner::ConversationPlanner;
use crate::handlers::session_manager::ConversationSessionManager;
use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};
use crate::prelude::XueliResult;
use crate::signals::engagement::build_message_observations;
use crate::signals::temporal::{build_temporal_context, normalize_event_time};
use crate::traits::ai_client::AIClient;

const DEFAULT_UNIFIED_FETCH_LIMIT: usize = 200;

use crate::traits::prompt_template::PromptTemplateLoader;

/// 会话计划协调器 — 协调群聊历史、视觉增强和规划器调用。
///
/// 它是群聊消息处理的核心调度枢纽，负责：构建消息上下文、调用规划器、记录对话历史。
pub struct ConversationPlanCoordinator<
    A: AIClient + 'static,
    L: PromptTemplateLoader + 'static = crate::services::prompt_loader::NoopPromptTemplateLoader,
> {
    pub planner: Arc<ConversationPlanner<A, L>>,
    pub session_manager: Arc<ConversationSessionManager>,
    pub context_builder: Arc<ConversationContextBuilder<L, A>>,
    conversation_store: Option<Arc<SqliteConversationStore>>,
    character_card_service: Option<Arc<CharacterCardService>>,
    /// 上下文窗口大小
    context_window_size: usize,
    /// 助手名称
    assistant_name: String,
}

/// 消息上下文 — 跨规划和回复生成共享的每条消息上下文
///
/// 对应 Python 版 `MessageContext`
#[derive(Debug, Clone)]
pub struct MessageContext {
    pub user_message: String,
    pub display_user_message: String,
    pub current_sender_label: String,
    pub conversation_key: String,
    pub is_first_turn: bool,
    pub current_event_time: f64,
    pub previous_message_time: f64,
    pub conversation_last_time: f64,
    pub temporal_context: crate::signals::temporal::TemporalContext,
    pub window_messages: Vec<HashMap<String, serde_json::Value>>,
    pub unified_history: Vec<HashMap<String, serde_json::Value>>,
    pub vision_analysis: HashMap<String, serde_json::Value>,
    pub planning_signals: HashMap<String, serde_json::Value>,
    pub relationship_summary: Option<String>,
    pub narrative_thread_summary: Option<String>,
    pub drive_context: Option<String>,
    pub soft_uncertainty_signals: Vec<String>,
    pub final_style_guide: Option<String>,
    pub session_restore_context: Option<String>,
    pub precise_recall_context: Option<String>,
    pub rendered_memory_sections: HashMap<String, String>,
    pub user_emotion_label: Option<String>,
    pub prompt_plan: Option<PromptPlan>,
}

impl Default for MessageContext {
    fn default() -> Self {
        Self {
            user_message: String::new(),
            display_user_message: String::new(),
            current_sender_label: String::new(),
            conversation_key: String::new(),
            is_first_turn: true,
            current_event_time: 0.0,
            previous_message_time: 0.0,
            conversation_last_time: 0.0,
            temporal_context: crate::signals::temporal::TemporalContext::new(),
            window_messages: Vec::new(),
            unified_history: Vec::new(),
            vision_analysis: HashMap::new(),
            planning_signals: HashMap::new(),
            relationship_summary: None,
            narrative_thread_summary: None,
            drive_context: None,
            soft_uncertainty_signals: Vec::new(),
            final_style_guide: None,
            session_restore_context: None,
            precise_recall_context: None,
            rendered_memory_sections: HashMap::new(),
            user_emotion_label: None,
            prompt_plan: None,
        }
    }
}

impl<A: AIClient, L: PromptTemplateLoader + 'static> ConversationPlanCoordinator<A, L> {
    pub fn new(
        planner: Arc<ConversationPlanner<A, L>>,
        session_manager: Arc<ConversationSessionManager>,
        context_builder: Arc<ConversationContextBuilder<L, A>>,
        assistant_name: impl Into<String>,
    ) -> Self {
        Self {
            planner,
            session_manager,
            context_builder,
            conversation_store: None,
            character_card_service: None,
            context_window_size: 10,
            assistant_name: assistant_name.into(),
        }
    }

    /// 设置对话存储（用于记录历史）
    pub fn with_conversation_store(mut self, store: Arc<SqliteConversationStore>) -> Self {
        self.conversation_store = Some(store);
        self
    }

    /// 设置角色卡服务
    pub fn with_character_card_service(mut self, svc: Arc<CharacterCardService>) -> Self {
        self.character_card_service = Some(svc);
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

    // ── 主协调入口 ──

    /// 主协调入口：构建消息上下文并调用规划器
    pub async fn coordinate(&self, event: &InboundEvent) -> XueliResult<MessageHandlingPlan> {
        let user_message = event
            .message
            .as_ref()
            .map(|m| m.text.clone())
            .unwrap_or_default();

        // a. 构建消息上下文
        let msg_ctx = self
            .build_message_context(event, &user_message)
            .await
            .unwrap_or_default();

        let conversation_key = self.history_key(event);

        let default_plan = ReplyPlan {
            id: String::new(),
            target_message_id: String::new(),
            topic: None,
            style: None,
            memory_recall_needed: false,
            use_emoji: false,
            priority: 0,
        };
        let mut context = self.context_builder.build(event, &default_plan).await?;

        // b. 将协调器层构建的上下文（时间、视觉、信号等）补充到 ConversationContext
        self.enrich_context_from_message_context(&mut context, &msg_ctx);

        // c. 调用规划器
        let plan = self.planner.plan(event, &context).await?;

        tracing::debug!(
            "[计划协调器] conversation={} should_reply={}",
            conversation_key,
            plan.should_reply
        );

        // d. 加载角色卡（若已配置）
        if self.character_card_service.is_some() && plan.should_reply {
            let user_id = event
                .message
                .as_ref()
                .map(|m| m.sender_id.clone())
                .unwrap_or_default();
            if !user_id.is_empty() {
                let _ = self
                    .character_card_service
                    .as_ref()
                    .unwrap()
                    .get_snapshot(&user_id);
            }
        }

        Ok(plan)
    }

    /// 将 plan_coordinator 构建的 MessageContext 中的视觉/时间/信号等补充到 ConversationContext
    fn enrich_context_from_message_context(
        &self,
        context: &mut ConversationContext,
        msg_ctx: &MessageContext,
    ) {
        if context.continuity_hint.is_none() && !msg_ctx.temporal_context.summary_text.is_empty() {
            context.continuity_hint = Some(msg_ctx.temporal_context.summary_text.clone());
        }
        if context.vision_description.is_none() {
            if let Some(merged) = msg_ctx
                .vision_analysis
                .get("merged_description")
                .and_then(|v| v.as_str())
            {
                context.vision_description = Some(merged.to_string());
            } else if !msg_ctx.vision_analysis.is_empty() {
                // 兜底：把 vision_analysis 整体序列化
                context.vision_description = Some(format!(
                    "[图片] {}",
                    serde_json::to_string(&msg_ctx.vision_analysis).unwrap_or_default()
                ));
            }
        }
        if context.narrative_thread_summary.is_none() && msg_ctx.narrative_thread_summary.is_some()
        {
            context.narrative_thread_summary = msg_ctx.narrative_thread_summary.clone();
        }
        if context.user_emotion_label.is_none() && msg_ctx.user_emotion_label.is_some() {
            context.user_emotion_label = msg_ctx.user_emotion_label.clone();
        }
    }

    /// 记录助手的回复到对话历史
    pub async fn record_assistant_reply(
        &self,
        event: &InboundEvent,
        reply_text: &str,
    ) -> XueliResult<()> {
        self.record_assistant_reply_with_group_id(event, reply_text, None)
            .await
    }

    /// 记录助手的回复到对话历史（支持显式 group_id）
    ///
    /// 对应 Python 版 `record_assistant_reply()`
    pub async fn record_assistant_reply_with_group_id(
        &self,
        event: &InboundEvent,
        reply_text: &str,
        _group_id: Option<&str>,
    ) -> XueliResult<()> {
        let text = reply_text.trim();
        if text.is_empty() {
            return Ok(());
        }

        let conversation_key = self.history_key(event);
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

    // ── 消息上下文构建 ──

    /// 构建消息上下文 — 加载历史、时间、信号、关系等所有上下文信息
    ///
    /// 对应 Python 版 `build_message_context()`
    pub async fn build_message_context(
        &self,
        event: &InboundEvent,
        user_message: &str,
    ) -> XueliResult<MessageContext> {
        let history_key = self.history_key(event);
        let conversation_key = history_key.clone();

        // 加载近期历史
        let history_items = self
            .get_recent_history(&history_key, 1, None)
            .await
            .unwrap_or_default();
        let unified_history_items = self
            .get_recent_history(&history_key, 1, Some(DEFAULT_UNIFIED_FETCH_LIMIT))
            .await
            .unwrap_or_default();

        // 构建当前消息条目
        let current_message = self.build_current_message(event, user_message);

        // 组合窗口消息
        let window_messages = self.compose_window_messages(&history_items, &current_message, false);

        // 获取会话（含恢复检测）
        let state = self.session_manager.get_or_create(&conversation_key).await;
        let has_restored = state.restored_session_pending;

        let (final_history_items, all_window_messages) = if has_restored {
            // 构建恢复消息并合并
            let msgs = self
                .session_manager
                .get_recent_messages(&conversation_key, 1000)
                .await;
            let restored_messages: Vec<HashMap<String, serde_json::Value>> = msgs
                .iter()
                .filter(|m| m.restored)
                .map(|m| {
                    let mut hm = HashMap::new();
                    hm.insert(
                        "message_id".to_string(),
                        serde_json::json!(m.message_id.clone()),
                    );
                    hm.insert("text".to_string(), serde_json::json!(m.content.clone()));
                    hm.insert(
                        "display_text".to_string(),
                        serde_json::json!(m.content.clone()),
                    );
                    hm.insert(
                        "text_content".to_string(),
                        serde_json::json!(m.content.clone()),
                    );
                    hm.insert("event_time".to_string(), serde_json::json!(m.timestamp));
                    hm.insert(
                        "has_image".to_string(),
                        serde_json::json!(!m.image_description.is_empty()),
                    );
                    hm.insert(
                        "raw_has_image".to_string(),
                        serde_json::json!(!m.image_description.is_empty()),
                    );
                    if !m.image_description.is_empty() {
                        hm.insert(
                            "per_image_descriptions".to_string(),
                            serde_json::json!(vec![m.image_description.clone()]),
                        );
                        hm.insert(
                            "merged_description".to_string(),
                            serde_json::json!(m.image_description.clone()),
                        );
                    }
                    hm.insert(
                        "speaker_role".to_string(),
                        serde_json::json!(m.role.clone()),
                    );
                    hm.insert("is_restored".to_string(), serde_json::json!(true));
                    hm.insert("is_latest".to_string(), serde_json::json!(false));
                    hm
                })
                .collect();

            let merged_items = Self::merge_history_items(&history_items, &restored_messages, 1);
            let _merged_window =
                self.compose_window_messages(&merged_items, &current_message, false);

            let mut all_restored = restored_messages;
            all_restored.sort_by(|a, b| {
                let ta = a.get("event_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let tb = b.get("event_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
                ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
            });

            let mut unified = unified_history_items
                .iter()
                .chain(all_restored.iter())
                .cloned()
                .collect::<Vec<_>>();
            unified.sort_by(|a, b| {
                let ta = a.get("event_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let tb = b.get("event_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
                ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
            });

            let all_window = self.compose_window_messages(&unified, &current_message, true);
            (merged_items, all_window)
        } else {
            let all_window =
                self.compose_window_messages(&unified_history_items, &current_message, true);
            (history_items.clone(), all_window)
        };

        // 时间上下文
        let previous_message_time = final_history_items
            .last()
            .and_then(|item| item.get("event_time"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let history_event_times: Vec<f64> = window_messages
            .iter()
            .filter_map(|item| item.get("event_time").and_then(|v| v.as_f64()))
            .collect();

        let is_group = event
            .message
            .as_ref()
            .map(|m| m.scope.is_group())
            .unwrap_or(false);
        let chat_mode = if is_group { "group" } else { "private" };

        let temporal_context = build_temporal_context(
            current_message
                .get("event_time")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            chat_mode,
            previous_message_time,
            previous_message_time,
            0.0,
            &history_event_times,
        );

        // 规划信号
        let planning_signals = self.build_planning_signals(&window_messages, &temporal_context);

        // 关系摘要
        let relationship_summary = self
            .build_relationship_summary_from_event(event)
            .await
            .unwrap_or_default();
        let relationship_summary = if relationship_summary.is_empty() {
            None
        } else {
            Some(relationship_summary)
        };

        // 视觉分析
        let vision_analysis = Self::extract_vision_analysis(&current_message);

        // 统一历史
        let unified_history =
            Self::build_unified_history(&all_window_messages, &history_items, None);

        let user_message_text = current_message
            .get("text_content")
            .and_then(|v| v.as_str())
            .unwrap_or(user_message)
            .trim()
            .to_string();

        let display_text = window_display_text_hm(&current_message);
        let sender_label = self
            .format_window_speaker(&current_message)
            .replace("用户 ", "")
            .trim()
            .to_string();

        Ok(MessageContext {
            user_message: user_message_text,
            display_user_message: display_text,
            current_sender_label: sender_label,
            conversation_key,
            is_first_turn: final_history_items.is_empty(),
            current_event_time: current_message
                .get("event_time")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            previous_message_time,
            conversation_last_time: previous_message_time,
            temporal_context,
            window_messages,
            unified_history,
            vision_analysis,
            planning_signals,
            relationship_summary,
            narrative_thread_summary: None,
            drive_context: None,
            soft_uncertainty_signals: Vec::new(),
            final_style_guide: None,
            session_restore_context: None,
            precise_recall_context: None,
            rendered_memory_sections: HashMap::new(),
            user_emotion_label: None,
            prompt_plan: None,
        })
    }

    // ── 格式化方法 ──

    /// 格式化窗口消息为提示词上下文
    ///
    /// 对应 Python 版 `format_window_context()`
    pub fn format_window_context(
        &self,
        window_messages: &[HashMap<String, serde_json::Value>],
    ) -> String {
        if window_messages.is_empty() {
            return String::new();
        }

        let mut lines = vec![
            "=== 当前群聊最近上下文（按时间顺序）===".to_string(),
            "如果你决定回复，请结合上下文自然接话。最近记录里可能包含助手自己之前的发言。"
                .to_string(),
        ];
        for (index, item) in window_messages.iter().enumerate() {
            let speaker = self.format_window_speaker(item);
            let text = window_display_text_hm(item);
            let latest_note = if item
                .get("is_latest")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                " [当前消息]"
            } else {
                ""
            };
            lines.push(format!("{}. {}{}{}", index + 1, speaker, text, latest_note));
        }
        lines.push(String::new());
        lines.join("\n")
    }

    /// 格式化窗口发言者标签
    fn format_window_speaker(&self, item: &HashMap<String, serde_json::Value>) -> String {
        let role = item
            .get("speaker_role")
            .and_then(|v| v.as_str())
            .unwrap_or("user")
            .trim()
            .to_lowercase();

        if role == "assistant" {
            let name = item
                .get("speaker_name")
                .and_then(|v| v.as_str())
                .unwrap_or(&self.assistant_name)
                .trim()
                .to_string();
            if name.is_empty() {
                format!("助手 {}：", self.assistant_name)
            } else {
                format!("助手 {}：", name)
            }
        } else {
            let user_id = item
                .get("user_id")
                .or_else(|| item.get("speaker_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .trim()
                .to_string();
            let display_name = item
                .get("speaker_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if !display_name.is_empty() && display_name != user_id {
                format!("用户 {}({})：", user_id, display_name)
            } else {
                format!("用户 {}：", user_id)
            }
        }
    }

    // ── 统一历史构建 ──

    /// 合并窗口消息 + 对话历史 + 关联历史为统一列表，按时间排序并去重
    ///
    /// 对应 Python 版 `build_unified_history()`
    pub fn build_unified_history(
        window_messages: &[HashMap<String, serde_json::Value>],
        history_items: &[HashMap<String, serde_json::Value>],
        _related_history: Option<&[HashMap<String, serde_json::Value>]>,
    ) -> Vec<HashMap<String, serde_json::Value>> {
        let mut result: Vec<HashMap<String, serde_json::Value>> = Vec::new();

        // 添加窗口消息
        for item in window_messages {
            let ts = item
                .get("event_time")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let role = item
                .get("speaker_role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .trim()
                .to_lowercase();
            let text = window_display_text_hm(item);
            let speaker_label = speaker_label(item);
            let content = if role == "user" {
                format!("{}：{}", speaker_label, text)
            } else {
                text
            };

            let mut entry = HashMap::new();
            entry.insert(
                "role".to_string(),
                serde_json::json!(if role == "assistant" {
                    "assistant"
                } else {
                    "user"
                }),
            );
            entry.insert("content".to_string(), serde_json::json!(content));
            entry.insert("timestamp".to_string(), serde_json::json!(ts));
            entry.insert(
                "message_id".to_string(),
                serde_json::json!(item
                    .get("message_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")),
            );
            result.push(entry);
        }

        // 添加历史消息（去重）
        let existing_ids: std::collections::HashSet<String> = result
            .iter()
            .filter_map(|e| e.get("message_id").and_then(|v| v.as_str()))
            .map(|s| s.to_string())
            .collect();

        for item in history_items {
            let mid = item
                .get("message_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !mid.is_empty() && existing_ids.contains(mid) {
                continue;
            }
            let role = item
                .get("speaker_role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .trim()
                .to_lowercase();
            let text = window_display_text_hm(item);
            let ts = item
                .get("event_time")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            let mut entry = HashMap::new();
            entry.insert(
                "role".to_string(),
                serde_json::json!(if role == "assistant" {
                    "assistant"
                } else {
                    "user"
                }),
            );
            entry.insert("content".to_string(), serde_json::json!(text));
            entry.insert("timestamp".to_string(), serde_json::json!(ts));
            entry.insert("message_id".to_string(), serde_json::json!(mid));
            result.push(entry);
        }

        // 按时间戳排序
        result.sort_by(|a, b| {
            let ta = a.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tb = b.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
            ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
        });
        result
    }

    // ── 规划信号 ──

    /// 构建观察信号（参与度模式分析）
    ///
    /// 对应 Python 版 `_build_planning_signals()`
    fn build_planning_signals(
        &self,
        window_messages: &[HashMap<String, serde_json::Value>],
        temporal_context: &crate::signals::temporal::TemporalContext,
    ) -> HashMap<String, serde_json::Value> {
        let latest_message = window_messages
            .iter()
            .rev()
            .find(|item| {
                item.get("is_latest")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .unwrap_or_else(|| {
                // fallback 到最后一条
                if window_messages.is_empty() {
                    &EMPTY_HM_REF
                } else {
                    &window_messages[window_messages.len() - 1]
                }
            });

        let previous_message = window_messages
            .iter()
            .rev()
            .find(|item| {
                !item
                    .get("is_latest")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true)
            })
            .unwrap_or(&EMPTY_HM_REF);

        let latest_text = window_display_text_hm(latest_message);

        let observations = build_message_observations(
            &latest_text,
            latest_message
                .get("user_id")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            previous_message
                .get("speaker_role")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            previous_message
                .get("user_id")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            &temporal_context.recent_gap_bucket,
            window_messages.len().saturating_sub(1),
        );

        let assistant_turns = window_messages
            .iter()
            .filter(|item| {
                item.get("speaker_role")
                    .and_then(|v| v.as_str())
                    .map(|r| r.trim().to_lowercase() == "assistant")
                    .unwrap_or(false)
            })
            .count();

        let mut signals = HashMap::new();
        signals.insert(
            "message_length_bucket".to_string(),
            serde_json::json!(observations.message_length_bucket),
        );
        signals.insert(
            "is_short_message".to_string(),
            serde_json::json!(observations.is_short_message),
        );
        signals.insert(
            "is_light_response_candidate".to_string(),
            serde_json::json!(observations.is_light_response_candidate),
        );
        signals.insert(
            "is_continuation_candidate".to_string(),
            serde_json::json!(observations.is_continuation_candidate),
        );
        signals.insert(
            "assistant_replied_recently".to_string(),
            serde_json::json!(observations.assistant_replied_recently),
        );
        signals.insert(
            "follows_assistant_recently".to_string(),
            serde_json::json!(observations.follows_assistant_recently),
        );
        signals.insert(
            "same_user_continuation".to_string(),
            serde_json::json!(observations.same_user_continuation),
        );
        signals.insert(
            "recent_history_count".to_string(),
            serde_json::json!(observations.recent_history_count),
        );
        signals.insert(
            "latest_message_length".to_string(),
            serde_json::json!(observations.latest_message_length),
        );
        signals.insert(
            "assistant_turns_in_window".to_string(),
            serde_json::json!(assistant_turns),
        );
        signals
    }

    // ── 关系摘要 ──

    /// 构建关系摘要 — 从角色卡快照中提取关系信息
    ///
    /// 对应 Python 版 `_build_relationship_summary()`
    async fn build_relationship_summary_from_event(
        &self,
        event: &InboundEvent,
    ) -> XueliResult<String> {
        let user_id = event
            .message
            .as_ref()
            .map(|m| m.sender_id.clone())
            .unwrap_or_default();
        if user_id.is_empty() {
            return Ok(String::new());
        }

        let mut parts: Vec<String> = Vec::new();

        if let Some(ref ccs) = self.character_card_service {
            let snapshot = ccs.get_snapshot(&user_id);
            if snapshot.intimacy_level > 0.0 || !snapshot.relationship_stage.is_empty() {
                parts.extend(Self::format_snapshot_relationship(&snapshot));
            }
        }

        Ok(if parts.is_empty() {
            String::new()
        } else {
            parts.join("\n")
        })
    }

    /// 公开的关系摘要构建方法 — 根据 user_id 和 scope 生成关系摘要
    ///
    /// 新公开接口，便于下游模块直接调用获取关系摘要。
    pub async fn build_relationship_summary(
        &self,
        user_id: &str,
        _scope: &ChatScope,
    ) -> XueliResult<Option<String>> {
        if user_id.is_empty() {
            return Ok(None);
        }

        let mut parts: Vec<String> = Vec::new();

        if let Some(ref ccs) = self.character_card_service {
            let snapshot = ccs.get_snapshot(user_id);
            if snapshot.intimacy_level > 0.0 || !snapshot.relationship_stage.is_empty() {
                parts.extend(Self::format_snapshot_relationship(&snapshot));
            }
        }

        Ok(if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        })
    }

    /// 从角色卡快照格式化关系阶段和语气信息
    fn format_snapshot_relationship(snapshot: &CharacterCardSnapshot) -> Vec<String> {
        let mut parts: Vec<String> = Vec::new();

        let intimacy = snapshot.intimacy_level;
        let stage = if intimacy >= 0.8 {
            "挚友"
        } else if intimacy >= 0.5 {
            "朋友"
        } else if intimacy >= 0.2 {
            "熟人"
        } else {
            "陌生人"
        };
        parts.push(format!("关系阶段: {stage}（亲密度 {intimacy:.2}）"));

        if !snapshot.emotional_trend.is_empty() {
            parts.push(format!("近期情绪趋势: {}", snapshot.emotional_trend));
        }

        if !snapshot.tone_preferences.is_empty() {
            let prefs: Vec<&str> = snapshot
                .tone_preferences
                .iter()
                .map(|s| s.as_str())
                .collect();
            parts.push(format!("用户偏好: {}", prefs.join("，")));
        }

        if !snapshot.core_traits.is_empty() {
            let traits: Vec<&str> = snapshot.core_traits.iter().map(|s| s.as_str()).collect();
            parts.push(format!("已知用户特质: {}", traits.join("，")));
        }

        if !snapshot.bot_persona_hints.is_empty() {
            let hints: Vec<&str> = snapshot
                .bot_persona_hints
                .iter()
                .map(|s| s.as_str())
                .collect();
            parts.push(format!("角色适配提示: {}", hints.join("，")));
        }

        parts
    }

    // ── 内部辅助方法 ──

    /// 从 ConversationRecord 转换为 HashMap（用于窗口消息等）
    fn record_to_hashmap(record: &ConversationRecord) -> HashMap<String, serde_json::Value> {
        let role = if record.is_bot { "assistant" } else { "user" };
        let mut hm = HashMap::new();
        hm.insert(
            "message_id".to_string(),
            serde_json::json!(record.message_id),
        );
        hm.insert("user_id".to_string(), serde_json::json!(record.user_id));
        hm.insert("speaker_role".to_string(), serde_json::json!(role));
        hm.insert(
            "speaker_name".to_string(),
            serde_json::json!(record.sender_name),
        );
        hm.insert("text".to_string(), serde_json::json!(record.text));
        hm.insert("display_text".to_string(), serde_json::json!(record.text));
        hm.insert("text_content".to_string(), serde_json::json!(record.text));
        hm.insert("raw_text".to_string(), serde_json::json!(record.text));
        hm.insert(
            "event_time".to_string(),
            serde_json::json!(record.event_time),
        );
        hm.insert("has_image".to_string(), serde_json::json!(false));
        hm.insert("raw_has_image".to_string(), serde_json::json!(false));
        hm.insert(
            "image_context_enabled".to_string(),
            serde_json::json!(false),
        );
        hm.insert("image_count".to_string(), serde_json::json!(0));
        hm.insert("raw_image_count".to_string(), serde_json::json!(0));
        hm.insert(
            "text_present".to_string(),
            serde_json::json!(!record.text.is_empty()),
        );
        hm.insert("is_image_only".to_string(), serde_json::json!(false));
        hm.insert("message_shape".to_string(), serde_json::json!("text_only"));
        hm.insert(
            "image_file_ids".to_string(),
            serde_json::json!(Vec::<String>::new()),
        );
        hm.insert(
            "per_image_descriptions".to_string(),
            serde_json::json!(Vec::<String>::new()),
        );
        hm.insert("merged_description".to_string(), serde_json::json!(""));
        hm.insert("vision_available".to_string(), serde_json::json!(false));
        hm.insert("vision_failure_count".to_string(), serde_json::json!(0));
        hm.insert("vision_success_count".to_string(), serde_json::json!(0));
        hm.insert("vision_source".to_string(), serde_json::json!(""));
        hm.insert("vision_error".to_string(), serde_json::json!(""));
        hm
    }

    /// 从事件获取发送者显示名称
    fn sender_display_name(&self, event: &InboundEvent) -> String {
        event
            .message
            .as_ref()
            .map(|m| {
                if m.sender_name.is_empty() {
                    m.sender_id.clone()
                } else {
                    m.sender_name.clone()
                }
            })
            .unwrap_or_default()
    }

    /// 获取事件时间
    fn event_time(&self, event: &InboundEvent) -> f64 {
        event
            .message
            .as_ref()
            .map(|m| normalize_event_time(m.timestamp.timestamp() as f64))
            .unwrap_or_else(|| chrono::Utc::now().timestamp() as f64)
    }

    /// 构建当前消息条目
    fn build_current_message(
        &self,
        event: &InboundEvent,
        user_message: &str,
    ) -> HashMap<String, serde_json::Value> {
        let clean_text = user_message.trim();
        let display_name = self.sender_display_name(event);
        let event_time = self.event_time(event);

        let raw_text = event
            .message
            .as_ref()
            .map(|m| m.text.clone())
            .unwrap_or_default();
        let effective_text = if !raw_text.trim().is_empty() {
            raw_text.trim().to_string()
        } else if !clean_text.is_empty() {
            clean_text.to_string()
        } else {
            String::new()
        };

        let text_present = !clean_text.is_empty();
        let message_shape = if text_present {
            "text_only"
        } else {
            "text_only"
        };

        let planner_text = if clean_text.is_empty() {
            "用户发送了空文本".to_string()
        } else {
            clean_text.to_string()
        };

        let display_text = effective_text.clone();

        let mut hm = HashMap::new();
        hm.insert(
            "message_id".to_string(),
            serde_json::json!(event
                .message
                .as_ref()
                .map(|m| m.id.clone())
                .unwrap_or_default()),
        );
        hm.insert(
            "user_id".to_string(),
            serde_json::json!(event
                .message
                .as_ref()
                .map(|m| m.sender_id.clone())
                .unwrap_or_default()),
        );
        hm.insert("speaker_role".to_string(), serde_json::json!("user"));
        hm.insert("speaker_name".to_string(), serde_json::json!(display_name));
        hm.insert("text".to_string(), serde_json::json!(planner_text.clone()));
        hm.insert("display_text".to_string(), serde_json::json!(display_text));
        hm.insert(
            "text_content".to_string(),
            serde_json::json!(clean_text.to_string()),
        );
        hm.insert("raw_text".to_string(), serde_json::json!(effective_text));
        hm.insert("event_time".to_string(), serde_json::json!(event_time));
        hm.insert("has_image".to_string(), serde_json::json!(false));
        hm.insert("raw_has_image".to_string(), serde_json::json!(false));
        hm.insert(
            "image_context_enabled".to_string(),
            serde_json::json!(false),
        );
        hm.insert("image_count".to_string(), serde_json::json!(0));
        hm.insert("raw_image_count".to_string(), serde_json::json!(0));
        hm.insert("text_present".to_string(), serde_json::json!(text_present));
        hm.insert(
            "is_image_only".to_string(),
            serde_json::json!(message_shape == "image_only"),
        );
        hm.insert(
            "message_shape".to_string(),
            serde_json::json!(message_shape),
        );
        hm.insert(
            "image_file_ids".to_string(),
            serde_json::json!(Vec::<String>::new()),
        );
        hm.insert(
            "per_image_descriptions".to_string(),
            serde_json::json!(Vec::<String>::new()),
        );
        hm.insert("merged_description".to_string(), serde_json::json!(""));
        hm.insert("vision_available".to_string(), serde_json::json!(false));
        hm.insert("vision_failure_count".to_string(), serde_json::json!(0));
        hm.insert("vision_success_count".to_string(), serde_json::json!(0));
        hm.insert("vision_source".to_string(), serde_json::json!(""));
        hm.insert("vision_error".to_string(), serde_json::json!(""));
        hm
    }

    /// 提取视觉分析字段
    fn extract_vision_analysis(
        current_message: &HashMap<String, serde_json::Value>,
    ) -> HashMap<String, serde_json::Value> {
        let mut result = HashMap::new();
        result.insert(
            "per_image_descriptions".to_string(),
            current_message
                .get("per_image_descriptions")
                .cloned()
                .unwrap_or(serde_json::json!(Vec::<String>::new())),
        );
        result.insert(
            "merged_description".to_string(),
            current_message
                .get("merged_description")
                .cloned()
                .unwrap_or(serde_json::json!("")),
        );
        result.insert(
            "vision_success_count".to_string(),
            current_message
                .get("vision_success_count")
                .cloned()
                .unwrap_or(serde_json::json!(0)),
        );
        result.insert(
            "vision_failure_count".to_string(),
            current_message
                .get("vision_failure_count")
                .cloned()
                .unwrap_or(serde_json::json!(0)),
        );
        result.insert(
            "vision_source".to_string(),
            current_message
                .get("vision_source")
                .cloned()
                .unwrap_or(serde_json::json!("")),
        );
        result.insert(
            "vision_error".to_string(),
            current_message
                .get("vision_error")
                .cloned()
                .unwrap_or(serde_json::json!("")),
        );
        result.insert(
            "vision_available".to_string(),
            current_message
                .get("vision_available")
                .cloned()
                .unwrap_or(serde_json::json!(false)),
        );
        result
    }

    /// 获取近期历史消息
    async fn get_recent_history(
        &self,
        group_id: &str,
        reserve_slots: usize,
        fetch_limit: Option<usize>,
    ) -> XueliResult<Vec<HashMap<String, serde_json::Value>>> {
        let count = fetch_limit.unwrap_or_else(|| self.max_history_items(reserve_slots));
        if count == 0 {
            return Ok(Vec::new());
        }

        let store = match &self.conversation_store {
            Some(s) => s,
            None => return Ok(Vec::new()),
        };

        let is_private_key = group_id.contains(":private:");
        let records = if is_private_key {
            store.get_recent_by_session(group_id, count).await?
        } else {
            store.get_recent_by_session(group_id, count).await?
        };

        Ok(records.iter().map(Self::record_to_hashmap).collect())
    }

    /// 组合窗口消息
    fn compose_window_messages(
        &self,
        history_items: &[HashMap<String, serde_json::Value>],
        current_message: &HashMap<String, serde_json::Value>,
        skip_trim: bool,
    ) -> Vec<HashMap<String, serde_json::Value>> {
        let current_id = current_message
            .get("message_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        let mut window_messages: Vec<HashMap<String, serde_json::Value>> = Vec::new();
        for item in history_items {
            if !current_id.is_empty()
                && item
                    .get("message_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    == current_id
            {
                continue;
            }
            let mut copied = item.clone();
            copied.insert("is_latest".to_string(), serde_json::json!(false));
            window_messages.push(copied);
        }

        let mut latest = current_message.clone();
        latest.insert("is_latest".to_string(), serde_json::json!(true));
        window_messages.push(latest);

        if skip_trim {
            return window_messages;
        }
        self.trim_window_messages(window_messages)
    }

    /// 裁剪窗口消息到上下文窗口大小
    fn trim_window_messages(
        &self,
        window_messages: Vec<HashMap<String, serde_json::Value>>,
    ) -> Vec<HashMap<String, serde_json::Value>> {
        let limit = self.context_window_size;
        if limit == 0 {
            return Vec::new();
        }
        let len = window_messages.len();
        if len <= limit {
            return window_messages;
        }
        window_messages.into_iter().skip(len - limit).collect()
    }

    /// 获取最大历史条目数（扣除保留槽位）
    fn max_history_items(&self, reserve_slots: usize) -> usize {
        if self.context_window_size == 0 {
            return 0;
        }
        self.context_window_size.saturating_sub(reserve_slots)
    }

    /// 合并历史消息条目（去重、按时间排序）
    fn merge_history_items(
        primary: &[HashMap<String, serde_json::Value>],
        supplemental: &[HashMap<String, serde_json::Value>],
        _reserve_slots: usize,
    ) -> Vec<HashMap<String, serde_json::Value>> {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut merged: Vec<HashMap<String, serde_json::Value>> = Vec::new();

        for source in [primary, supplemental] {
            for item in source {
                let key = Self::history_item_key(item);
                if seen.contains(&key) {
                    continue;
                }
                seen.insert(key);
                merged.push(item.clone());
            }
        }

        merged.sort_by(|a, b| {
            let ta = a.get("event_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tb = b.get("event_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let cmp = ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal);
            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
            let ma = a.get("message_id").and_then(|v| v.as_str()).unwrap_or("");
            let mb = b.get("message_id").and_then(|v| v.as_str()).unwrap_or("");
            ma.cmp(mb)
        });

        merged
    }

    /// 生成历史条目去重键
    fn history_item_key(item: &HashMap<String, serde_json::Value>) -> String {
        let message_id = item
            .get("message_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if !message_id.is_empty() {
            return format!("message_id:{}", message_id);
        }
        let role = item
            .get("speaker_role")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let uid = item.get("user_id").and_then(|v| v.as_str()).unwrap_or("");
        let ts = item
            .get("event_time")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let text = item
            .get("text_content")
            .or_else(|| item.get("text"))
            .or_else(|| item.get("display_text"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        format!("fallback:{}:{}:{}:{}", role, uid, ts, text)
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

// ── 静态空 HashMap 引用（用于 fallback）──

/// 空 HashMap 的静态引用，用于计划信号构建中的 fallback
static EMPTY_HM: std::sync::LazyLock<HashMap<String, serde_json::Value>> =
    std::sync::LazyLock::new(HashMap::new);
static EMPTY_HM_REF: &std::sync::LazyLock<HashMap<String, serde_json::Value>> = &EMPTY_HM;

// ── 公共辅助函数 ──

/// 从 HashMap 消息项构建窗口显示文本（独立函数，供本模块和其他模块使用）
pub fn window_display_text_hm(item: &HashMap<String, serde_json::Value>) -> String {
    crate::handlers::shared::display_utils::window_display_text(item)
}

/// 生成发言者标签（独立函数，供统一历史构建使用）
pub fn speaker_label(item: &HashMap<String, serde_json::Value>) -> String {
    let role = item
        .get("speaker_role")
        .and_then(|v| v.as_str())
        .unwrap_or("user")
        .trim()
        .to_lowercase();

    if role == "assistant" {
        let name = item
            .get("speaker_name")
            .and_then(|v| v.as_str())
            .unwrap_or("assistant")
            .trim();
        return if name.is_empty() {
            "助手".to_string()
        } else {
            name.to_string()
        };
    }

    let speaker = item
        .get("speaker_name")
        .or_else(|| item.get("user_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("用户")
        .trim()
        .to_string();

    let user_id = item
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if !speaker.is_empty() && !user_id.is_empty() && speaker != user_id {
        format!("{}({})", speaker, user_id)
    } else if !speaker.is_empty() {
        speaker
    } else {
        user_id
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
            ..Default::default()
        }
    }

    #[test]
    fn test_history_key_group() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SqliteConversationStore::open(temp_dir.path()).unwrap());
        let builder = Arc::new(ConversationContextBuilder::<
            crate::services::prompt_loader::NoopPromptTemplateLoader,
        >::new(store));
        let planner = Arc::new(ConversationPlanner::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "test-model",
            "测试助手",
            "",
            "zh-CN",
        ));
        let session_mgr = Arc::new(ConversationSessionManager::new(None));

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
        let builder = Arc::new(ConversationContextBuilder::<
            crate::services::prompt_loader::NoopPromptTemplateLoader,
        >::new(store));
        let planner = Arc::new(ConversationPlanner::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "test-model",
            "测试助手",
            "",
            "zh-CN",
        ));
        let session_mgr = Arc::new(ConversationSessionManager::new(None));

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
            ..Default::default()
        };
        let key = coord.history_key(&event);
        assert!(key.contains("private"));
        assert!(key.contains("u2"));
    }

    #[test]
    fn test_record_to_hashmap() {
        let record = ConversationRecord {
            id: 1,
            session_id: "test:group:g1".into(),
            user_id: "u1".into(),
            sender_name: "Alice".into(),
            text: "你好".into(),
            is_bot: false,
            scope_type: "group".into(),
            scope_id: "g1".into(),
            event_time: 1000.0,
            message_id: "msg_1".into(),
            platform: "test".into(),
        };
        let hm = ConversationPlanCoordinator::<
            crate::services::ai_client::NoopAIClient,
        >::record_to_hashmap(&record);
        assert_eq!(
            hm.get("speaker_role").and_then(|v| v.as_str()).unwrap(),
            "user"
        );
        assert_eq!(hm.get("text").and_then(|v| v.as_str()).unwrap(), "你好");
    }

    #[test]
    fn test_build_current_message() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SqliteConversationStore::open(temp_dir.path()).unwrap());
        let builder = Arc::new(ConversationContextBuilder::<
            crate::services::prompt_loader::NoopPromptTemplateLoader,
        >::new(store));
        let planner = Arc::new(ConversationPlanner::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "test-model",
            "测试助手",
            "",
            "zh-CN",
        ));
        let session_mgr = Arc::new(ConversationSessionManager::new(None));

        let coord = ConversationPlanCoordinator::new(planner, session_mgr, builder, "测试助手");

        let event = make_event("g456");
        let msg = coord.build_current_message(&event, "你好世界");
        assert_eq!(
            msg.get("text_content").and_then(|v| v.as_str()).unwrap(),
            "你好世界"
        );
        assert_eq!(
            msg.get("speaker_role").and_then(|v| v.as_str()).unwrap(),
            "user"
        );
        assert_eq!(
            msg.get("message_shape").and_then(|v| v.as_str()).unwrap(),
            "text_only"
        );
    }

    #[test]
    fn test_build_current_message_empty() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SqliteConversationStore::open(temp_dir.path()).unwrap());
        let builder = Arc::new(ConversationContextBuilder::<
            crate::services::prompt_loader::NoopPromptTemplateLoader,
        >::new(store));
        let planner = Arc::new(ConversationPlanner::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "test-model",
            "测试助手",
            "",
            "zh-CN",
        ));
        let session_mgr = Arc::new(ConversationSessionManager::new(None));

        let coord = ConversationPlanCoordinator::new(planner, session_mgr, builder, "测试助手");

        let event = make_event("g456");
        let msg = coord.build_current_message(&event, "");
        assert_eq!(
            msg.get("text").and_then(|v| v.as_str()).unwrap(),
            "用户发送了空文本"
        );
    }

    #[test]
    fn test_build_unified_history() {
        let mut wm1 = HashMap::new();
        wm1.insert("speaker_role".to_string(), serde_json::json!("user"));
        wm1.insert("user_id".to_string(), serde_json::json!("u1"));
        wm1.insert("speaker_name".to_string(), serde_json::json!("Alice"));
        wm1.insert("text".to_string(), serde_json::json!("你好"));
        wm1.insert("event_time".to_string(), serde_json::json!(100.0));
        wm1.insert("message_id".to_string(), serde_json::json!("m1"));

        let mut wm2 = HashMap::new();
        wm2.insert("speaker_role".to_string(), serde_json::json!("assistant"));
        wm2.insert("speaker_name".to_string(), serde_json::json!("Bot"));
        wm2.insert("text".to_string(), serde_json::json!("你好呀"));
        wm2.insert("event_time".to_string(), serde_json::json!(110.0));
        wm2.insert("message_id".to_string(), serde_json::json!("m2"));

        let window = vec![wm1, wm2];
        let history: Vec<HashMap<String, serde_json::Value>> = Vec::new();

        let result = ConversationPlanCoordinator::<
            crate::services::ai_client::NoopAIClient,
        >::build_unified_history(&window, &history, None);
        assert_eq!(result.len(), 2);
        // 按时间戳排序
        let r0_role = result[0].get("role").and_then(|v| v.as_str()).unwrap();
        assert_eq!(r0_role, "user");
        let r1_role = result[1].get("role").and_then(|v| v.as_str()).unwrap();
        assert_eq!(r1_role, "assistant");
    }

    #[test]
    fn test_build_unified_history_empty() {
        let result = ConversationPlanCoordinator::<
            crate::services::ai_client::NoopAIClient,
        >::build_unified_history(&[], &[], None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_window_speaker() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SqliteConversationStore::open(temp_dir.path()).unwrap());
        let builder = Arc::new(ConversationContextBuilder::<
            crate::services::prompt_loader::NoopPromptTemplateLoader,
        >::new(store));
        let planner = Arc::new(ConversationPlanner::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "test-model",
            "测试助手",
            "",
            "zh-CN",
        ));
        let session_mgr = Arc::new(ConversationSessionManager::new(None));

        let coord = ConversationPlanCoordinator::new(planner, session_mgr, builder, "BotName");

        let mut user_item = HashMap::new();
        user_item.insert("speaker_role".to_string(), serde_json::json!("user"));
        user_item.insert("user_id".to_string(), serde_json::json!("u123"));
        user_item.insert("speaker_name".to_string(), serde_json::json!("张三"));
        let label = coord.format_window_speaker(&user_item);
        assert!(label.contains("张三"));

        let mut assistant_item = HashMap::new();
        assistant_item.insert("speaker_role".to_string(), serde_json::json!("assistant"));
        let label = coord.format_window_speaker(&assistant_item);
        assert!(label.contains("BotName"));
    }

    #[test]
    fn test_format_window_context() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SqliteConversationStore::open(temp_dir.path()).unwrap());
        let builder = Arc::new(ConversationContextBuilder::<
            crate::services::prompt_loader::NoopPromptTemplateLoader,
        >::new(store));
        let planner = Arc::new(ConversationPlanner::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "test-model",
            "测试助手",
            "",
            "zh-CN",
        ));
        let session_mgr = Arc::new(ConversationSessionManager::new(None));

        let coord = ConversationPlanCoordinator::new(planner, session_mgr, builder, "BotName");

        let mut msg = HashMap::new();
        msg.insert("speaker_role".to_string(), serde_json::json!("user"));
        msg.insert("user_id".to_string(), serde_json::json!("u1"));
        msg.insert("speaker_name".to_string(), serde_json::json!("Alice"));
        msg.insert("text".to_string(), serde_json::json!("你好"));
        msg.insert("is_latest".to_string(), serde_json::json!(true));

        let result = coord.format_window_context(&[msg]);
        assert!(result.contains("当前群聊最近上下文"));
        assert!(result.contains("你好"));
        assert!(result.contains("当前消息"));
    }

    #[test]
    fn test_format_window_context_empty() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SqliteConversationStore::open(temp_dir.path()).unwrap());
        let builder = Arc::new(ConversationContextBuilder::<
            crate::services::prompt_loader::NoopPromptTemplateLoader,
        >::new(store));
        let planner = Arc::new(ConversationPlanner::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "test-model",
            "测试助手",
            "",
            "zh-CN",
        ));
        let session_mgr = Arc::new(ConversationSessionManager::new(None));

        let coord = ConversationPlanCoordinator::new(planner, session_mgr, builder, "BotName");

        let result = coord.format_window_context(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_snapshot_relationship() {
        let snapshot = CharacterCardSnapshot {
            user_id: "u1".into(),
            core_traits: vec!["幽默".into(), "好奇".into()],
            tone_preferences: vec!["轻松".into()],
            intimacy_level: 0.85,
            emotional_trend: "积极".into(),
            ..Default::default()
        };

        let parts = ConversationPlanCoordinator::<
            crate::services::ai_client::NoopAIClient,
        >::format_snapshot_relationship(&snapshot);
        let joined = parts.join("\n");
        assert!(joined.contains("挚友"));
        assert!(joined.contains("0.85"));
        assert!(joined.contains("轻松"));
        assert!(joined.contains("幽默"));
        assert!(joined.contains("积极"));
    }

    #[test]
    fn test_build_planning_signals_no_messages() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SqliteConversationStore::open(temp_dir.path()).unwrap());
        let builder = Arc::new(ConversationContextBuilder::<
            crate::services::prompt_loader::NoopPromptTemplateLoader,
        >::new(store));
        let planner = Arc::new(ConversationPlanner::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "test-model",
            "测试助手",
            "",
            "zh-CN",
        ));
        let session_mgr = Arc::new(ConversationSessionManager::new(None));

        let coord = ConversationPlanCoordinator::new(planner, session_mgr, builder, "BotName");

        let temporal = crate::signals::temporal::TemporalContext::new();
        let signals = coord.build_planning_signals(&[], &temporal);
        assert!(!signals.is_empty());
        assert_eq!(
            signals
                .get("assistant_turns_in_window")
                .and_then(|v| v.as_u64())
                .unwrap(),
            0
        );
    }

    #[test]
    fn test_merge_history_items() {
        let mut item1 = HashMap::new();
        item1.insert("message_id".to_string(), serde_json::json!("m1"));
        item1.insert("event_time".to_string(), serde_json::json!(100.0));
        item1.insert("text".to_string(), serde_json::json!("first"));

        let mut item2 = HashMap::new();
        item2.insert("message_id".to_string(), serde_json::json!("m2"));
        item2.insert("event_time".to_string(), serde_json::json!(200.0));
        item2.insert("text".to_string(), serde_json::json!("second"));

        // duplicate of item1
        let mut item1_dup = HashMap::new();
        item1_dup.insert("message_id".to_string(), serde_json::json!("m1"));
        item1_dup.insert("event_time".to_string(), serde_json::json!(100.0));
        item1_dup.insert("text".to_string(), serde_json::json!("first"));

        let merged = ConversationPlanCoordinator::<
            crate::services::ai_client::NoopAIClient,
        >::merge_history_items(&[item1.clone(), item2.clone()], &[item1_dup], 10);
        // 应该去重：m1 只出现一次
        assert_eq!(merged.len(), 2);
        // 应该按时间排序
        assert_eq!(
            merged[0]
                .get("message_id")
                .and_then(|v| v.as_str())
                .unwrap(),
            "m1"
        );
        assert_eq!(
            merged[1]
                .get("message_id")
                .and_then(|v| v.as_str())
                .unwrap(),
            "m2"
        );
    }

    #[test]
    fn test_speaker_label_user_with_name_and_id() {
        let mut item = HashMap::new();
        item.insert("speaker_role".to_string(), serde_json::json!("user"));
        item.insert("user_id".to_string(), serde_json::json!("u123"));
        item.insert("speaker_name".to_string(), serde_json::json!("张三"));
        let label = speaker_label(&item);
        assert_eq!(label, "张三(u123)");
    }

    #[test]
    fn test_speaker_label_user_name_only() {
        let mut item = HashMap::new();
        item.insert("speaker_role".to_string(), serde_json::json!("user"));
        item.insert("speaker_name".to_string(), serde_json::json!("李四"));
        let label = speaker_label(&item);
        assert_eq!(label, "李四");
    }

    #[test]
    fn test_speaker_label_assistant() {
        let mut item = HashMap::new();
        item.insert("speaker_role".to_string(), serde_json::json!("assistant"));
        item.insert("speaker_name".to_string(), serde_json::json!("雪梨"));
        let label = speaker_label(&item);
        assert_eq!(label, "雪梨");
    }

    #[test]
    fn test_window_display_text_hm() {
        let mut item = HashMap::new();
        item.insert("text".to_string(), serde_json::json!("测试消息"));
        let result = window_display_text_hm(&item);
        assert_eq!(result, "测试消息");
    }

    #[test]
    fn test_history_item_key_with_message_id() {
        let mut item = HashMap::new();
        item.insert("message_id".to_string(), serde_json::json!("msg_123"));
        let key = ConversationPlanCoordinator::<
            crate::services::ai_client::NoopAIClient,
        >::history_item_key(&item);
        assert_eq!(key, "message_id:msg_123");
    }

    #[test]
    fn test_history_item_key_fallback() {
        let mut item = HashMap::new();
        item.insert("speaker_role".to_string(), serde_json::json!("user"));
        item.insert("user_id".to_string(), serde_json::json!("u1"));
        item.insert("event_time".to_string(), serde_json::json!(123.45));
        item.insert("text".to_string(), serde_json::json!("hi"));
        let key = ConversationPlanCoordinator::<
            crate::services::ai_client::NoopAIClient,
        >::history_item_key(&item);
        assert!(key.starts_with("fallback:"));
        assert!(key.contains("u1"));
    }
}
