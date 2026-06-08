use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::scope::ChatScope;

/// 用户消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    /// 消息唯一 ID
    pub id: String,
    /// 发送者 ID
    pub sender_id: String,
    /// 发送者昵称
    pub sender_name: String,
    /// 消息文本内容
    pub text: String,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 所在作用域
    pub scope: ChatScope,
    /// 是否 @了 bot
    pub is_mention: bool,
}

/// 会话信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    /// 会话 ID
    pub id: String,
    /// 作用域
    pub scope: ChatScope,
    /// 参与者 ID 列表
    pub participants: Vec<String>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后活跃时间
    pub last_active_at: DateTime<Utc>,
    /// 消息数
    pub message_count: u64,
}

/// 情绪状态 — 多维连续值模型
///
/// - `valence`: 愉悦度，-1.0（负面）到 1.0（正面）
/// - `energy`: 精力，0.0（枯竭）到 1.0（充沛）
/// - `arousal`: 唤醒度，0.0（平静）到 1.0（激动）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MoodState {
    pub valence: f64,
    pub energy: f64,
    pub arousal: f64,
    pub updated_at: String,
    #[serde(default)]
    pub mood_cycle_day: i32,
}

impl MoodState {
    pub fn new() -> Self {
        Self {
            valence: 0.0,
            energy: 0.8,
            arousal: 0.5,
            updated_at: String::new(),
            mood_cycle_day: 0,
        }
    }
}

impl Default for MoodState {
    fn default() -> Self {
        Self::new()
    }
}

/// 关系亲密度档案 — 机器人与用户之间的长期关系动态
///
/// 对应 Python 版 `src.core.models.RelationshipProfile`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipProfile {
    pub user_id: String,
    pub intimacy_level: f64,
    #[serde(default)]
    pub relationship_stage: String,
    #[serde(default)]
    pub last_intimacy_change: String,
    #[serde(default)]
    pub friction_signals: i32,
    #[serde(default)]
    pub total_interactions: i32,
    #[serde(default)]
    pub interactions_last_week: i32,
    #[serde(default)]
    pub deep_conversation_ratio: f64,
    #[serde(default)]
    pub topic_overlap_count: i32,
    #[serde(default)]
    pub last_interaction_at: String,
    #[serde(default)]
    pub reply_positive_count: i32,
    #[serde(default)]
    pub reply_negative_count: i32,
    #[serde(default)]
    pub reply_repair_count: i32,
    #[serde(default)]
    pub last_feedback_label: String,
    #[serde(default)]
    pub last_reply_intent: String,
}

impl RelationshipProfile {
    pub fn resolve_stage(&self) -> &str {
        if self.intimacy_level >= 0.9 {
            "intimate"
        } else if self.intimacy_level >= 0.8 {
            "close_friend"
        } else if self.intimacy_level >= 0.5 {
            "friend"
        } else if self.intimacy_level >= 0.2 {
            "acquaintance"
        } else if self.intimacy_level >= 0.1 {
            "met_before"
        } else {
            "stranger"
        }
    }
}

impl Default for RelationshipProfile {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            intimacy_level: 0.0,
            relationship_stage: "stranger".to_string(),
            last_intimacy_change: String::new(),
            friction_signals: 0,
            total_interactions: 0,
            interactions_last_week: 0,
            deep_conversation_ratio: 0.0,
            topic_overlap_count: 0,
            last_interaction_at: String::new(),
            reply_positive_count: 0,
            reply_negative_count: 0,
            reply_repair_count: 0,
            last_feedback_label: String::new(),
            last_reply_intent: String::new(),
        }
    }
}

/// 回复计划
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyPlan {
    /// 计划 ID
    pub id: String,
    /// 目标消息
    pub target_message_id: String,
    /// 回复主题
    pub topic: Option<String>,
    /// 回复风格
    pub style: Option<String>,
    /// 需要回忆的记忆
    pub memory_recall_needed: bool,
    /// 是否使用 emoji
    pub use_emoji: bool,
    /// 回复优先级
    pub priority: i32,
}

/// 记忆条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    /// 记忆 ID
    pub id: String,
    /// 所属用户 ID
    pub user_id: String,
    /// 记忆内容
    pub content: String,
    /// 记忆类型
    pub memory_type: MemoryType,
    /// 重要度 (0.0 - 1.0)
    pub importance: f64,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后访问时间
    pub last_accessed_at: DateTime<Utc>,
    /// 访问次数
    pub access_count: u64,
}

/// 记忆类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryType {
    /// 事实信息
    Fact,
    /// 偏好
    Preference,
    /// 事件经历
    Event,
    /// 观点态度
    Opinion,
    /// 关系信息
    Relationship,
}

/// 提取的记忆 Patch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPatch {
    /// 新增记忆
    pub add: Vec<MemoryItem>,
    /// 更新记忆
    pub update: Vec<MemoryItem>,
    /// 删除记忆 ID
    pub remove: Vec<String>,
}

/// 不可变消息 — 消息一旦创建，内容和时间均不可变
///
/// 对应 Python 版 `src.core.models.ImmutableMessage`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImmutableMessage {
    pub message_id: String,
    pub user_id: String,
    pub content: String,
    /// 事件时间（Unix 时间戳，秒）
    pub event_time: f64,
    /// 接收时间（Unix 时间戳，秒）
    pub received_time: f64,
    /// 原始数据（JSON）
    pub raw_data: serde_json::Value,
    /// 显示名称
    pub display_name: String,
    /// 原始事件文本
    pub event_text: String,
}

/// 会话快照 — 基于时间点的静态群聊上下文视图
///
/// 对应 Python 版 `src.core.models.ConversationSnapshot`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    pub conversation_id: String,
    pub messages: Vec<ImmutableMessage>,
    pub snapshot_time: f64,
    pub created_at: f64,
}

/// 跨时间线、记忆和引用层共享的结构化上下文条目。
///
/// 对应 Python 版 `src.core.models.ConversationContextItem`
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// 动作规划后生成的结构化提示词策略
///
/// 对应 Python 版 `src.core.models.PromptPlan`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptPlan {
    #[serde(default)]
    pub reply_goal: String,
    #[serde(default)]
    pub continuity_mode: String,
    #[serde(default)]
    pub personality_mode: String,
    #[serde(default)]
    pub emoji_should_send: bool,
    #[serde(default)]
    pub emoji_instruction: String,
    #[serde(default)]
    pub conversation_style: String,
    #[serde(default)]
    pub mood_instruction: String,
    #[serde(default)]
    pub planner_reminder: String,
    #[serde(default)]
    pub policy: PromptSectionPolicy,
    #[serde(default)]
    pub notes: String,
}

impl Default for PromptPlan {
    fn default() -> Self {
        Self {
            reply_goal: "continue".to_string(),
            continuity_mode: "direct_continue".to_string(),
            personality_mode: "balanced".to_string(),
            emoji_should_send: false,
            emoji_instruction: String::new(),
            conversation_style: "standard".to_string(),
            mood_instruction: String::new(),
            planner_reminder: String::new(),
            policy: PromptSectionPolicy::default(),
            notes: String::new(),
        }
    }
}

/// 规划器控制的提示词区段编译开关
///
/// 对应 Python 版 `src.core.models.PromptSectionPolicy`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSectionPolicy {
    #[serde(default = "default_true")]
    pub include_recent_history: bool,
    #[serde(default = "default_true")]
    pub include_person_facts: bool,
    #[serde(default = "default_true")]
    pub include_session_restore: bool,
    #[serde(default = "default_true")]
    pub include_precise_recall: bool,
    #[serde(default = "default_true")]
    pub include_dynamic_memory: bool,
    #[serde(default = "default_true")]
    pub include_vision_context: bool,
    #[serde(default = "default_true")]
    pub include_reply_scope: bool,
    #[serde(default = "default_true")]
    pub include_style_guide: bool,
}

fn default_true() -> bool {
    true
}

impl Default for PromptSectionPolicy {
    fn default() -> Self {
        Self {
            include_recent_history: true,
            include_person_facts: true,
            include_session_restore: true,
            include_precise_recall: true,
            include_dynamic_memory: true,
            include_vision_context: true,
            include_reply_scope: true,
            include_style_guide: true,
        }
    }
}

/// 最终可见回复风格的结构化指引
///
/// 对应 Python 版 `src.core.models.FinalStyleGuide`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FinalStyleGuide {
    #[serde(default)]
    pub verbosity_guidance: String,
    #[serde(default)]
    pub warmth_guidance: String,
    #[serde(default)]
    pub initiative_guidance: String,
    #[serde(default)]
    pub tone_guidance: String,
    #[serde(default)]
    pub expression_guidance: String,
    #[serde(default)]
    pub opening_style: String,
    #[serde(default)]
    pub sentence_shape: String,
    #[serde(default)]
    pub followup_shape: String,
    #[serde(default)]
    pub allowed_colloquialism: String,
    #[serde(default)]
    pub relationship_guidance: String,
    #[serde(default)]
    pub anti_patterns: Vec<String>,
}

/// 等待窗口/消息收集层发出的收集到的消息
///
/// 对应 Python 版 `src.core.models.PlanningWindowResult`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlanningWindowResult {
    #[serde(default)]
    pub merged_user_message: String,
    #[serde(default)]
    pub window_messages: Vec<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub planning_signals: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub window_reason: String,
    #[serde(default)]
    pub bypassed: bool,
}

/// 记忆冲突反思的归一化后台判决
///
/// 对应 Python 版 `src.core.models.MemoryDisputeDecision`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryDisputeDecision {
    #[serde(default)]
    pub dispute_id: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub resolution: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub conflict_type: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub merged_content: Option<String>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub targets: Vec<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub evidence: Vec<HashMap<String, serde_json::Value>>,
}

/// 记忆争议的结构化持久化证据
///
/// 对应 Python 版 `src.core.models.FactEvidenceRecord`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FactEvidenceRecord {
    pub record_id: String,
    pub user_id: String,
    #[serde(default)]
    pub fact_id: String,
    #[serde(default)]
    pub source_memory_id: String,
    #[serde(default)]
    pub source_memory_type: String,
    #[serde(default)]
    pub evidence_type: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub decision_level: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub conflict_type: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub ttl_seconds: Option<i64>,
    #[serde(default)]
    pub targets: Vec<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub evidence: Vec<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// 源自高置信度记忆争议的谨慎语气信号
///
/// 对应 Python 版 `src.core.models.SoftUncertaintySignal`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SoftUncertaintySignal {
    pub signal_id: String,
    pub user_id: String,
    #[serde(default)]
    pub uncertainty_type: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub conflict_type: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_memory_id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub expires_at: String,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// 消息处理计划
///
/// 对应 Python 版 `src.core.models.MessageHandlingPlan`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageHandlingPlan {
    pub action: String,
    pub reason: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub should_reply: bool,
    #[serde(default)]
    pub raw_decision: Option<serde_json::Value>,
    #[serde(default)]
    pub reply_context: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub prompt_plan: Option<PromptPlan>,
    #[serde(default)]
    pub reply_reference: String,
    #[serde(default)]
    pub planning_signals: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub planner_caution_hint: Option<String>,
    #[serde(default)]
    pub risk_posture: Option<String>,
    #[serde(default)]
    pub cached_context: Option<String>,
}
