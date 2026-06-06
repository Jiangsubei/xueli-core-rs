use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::character::card_service::CharacterCardSnapshot;
use crate::core::types::Conversation;
use crate::handlers::planner::PromptPlan;
use crate::handlers::reply::style_policy::{FinalStyleGuide, SoftUncertaintySignal};
use crate::signals::temporal::TemporalContext;

/// 跨规划和回复生成共享的统一每条消息上下文
///
/// 对应 Python 版 `src.handlers.conversation.context.MessageContext`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageContext {
    pub trace_id: String,
    pub execution_key: String,
    pub conversation_key: String,
    pub user_message: String,
    pub display_user_message: String,
    pub current_sender_label: String,
    pub is_first_turn: bool,
    pub current_event_time: f64,
    pub previous_message_time: f64,
    pub conversation_last_time: f64,
    pub previous_session_time: f64,
    pub temporal_context: TemporalContext,
    pub context_items: Vec<ConversationContextItem>,
    pub window_messages: Vec<HashMap<String, serde_json::Value>>,
    pub unified_history: Vec<HashMap<String, serde_json::Value>>,
    pub recent_history_text: String,
    pub rendered_recent_history: String,
    pub rendered_timeline_summary: String,
    pub rendered_memory_sections: HashMap<String, String>,
    pub base64_images: Vec<String>,
    pub vision_analysis: HashMap<String, serde_json::Value>,
    pub person_fact_context: String,
    pub persistent_memory_context: String,
    pub session_restore_context: String,
    pub precise_recall_context: String,
    pub dynamic_memory_context: String,
    pub related_history_messages: Vec<HashMap<String, serde_json::Value>>,
    pub reply_context: HashMap<String, serde_json::Value>,
    pub direct_reply_text: String,
    pub planning_signals: HashMap<String, serde_json::Value>,
    pub soft_uncertainty_signals: Vec<SoftUncertaintySignal>,
    pub character_card_snapshot: CharacterCardSnapshot,
    pub user_emotion_label: String,
    pub mood_state: HashMap<String, serde_json::Value>,
    pub user_profile_signal: HashMap<String, serde_json::Value>,
    pub mood_decision_signal: HashMap<String, serde_json::Value>,
    pub conversation_window_signal: HashMap<String, serde_json::Value>,
    pub style_adaptation_signal: HashMap<String, serde_json::Value>,
    pub relationship_state_signal: HashMap<String, serde_json::Value>,
    pub caution_signal: HashMap<String, serde_json::Value>,
    pub metacognition_state_report: String,
    pub narrative_thread_summary: String,
    pub narrative_thread_label: String,
    pub narrative_self: HashMap<String, serde_json::Value>,
    pub window_reason: String,
    pub relationship_summary: String,
    pub system_state_block: String,
    pub prompt_plan: Option<PromptPlan>,
    pub reply_reference: String,
    pub final_style_guide: FinalStyleGuide,
    pub conversation: Option<Conversation>,
}

/// 跨时间线、记忆和引用层共享的结构化上下文条目
///
/// 对应 Python 版 `src.core.models.ConversationContextItem`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationContextItem {
    pub kind: String,
    pub text: String,
    pub role: String,
    pub speaker_label: String,
    pub timestamp: f64,
    pub metadata: HashMap<String, serde_json::Value>,
    pub count_in_context: bool,
}

impl ConversationContextItem {
    pub fn new(kind: &str, text: &str) -> Self {
        Self {
            kind: kind.to_string(),
            text: text.to_string(),
            role: "user".to_string(),
            speaker_label: String::new(),
            timestamp: 0.0,
            metadata: HashMap::new(),
            count_in_context: true,
        }
    }

    pub fn with_role(mut self, role: &str) -> Self {
        self.role = role.to_string();
        self
    }

    pub fn with_speaker_label(mut self, label: &str) -> Self {
        self.speaker_label = label.to_string();
        self
    }

    pub fn with_timestamp(mut self, timestamp: f64) -> Self {
        self.timestamp = timestamp;
        self
    }

    pub fn with_count_in_context(mut self, count: bool) -> Self {
        self.count_in_context = count;
        self
    }
}
