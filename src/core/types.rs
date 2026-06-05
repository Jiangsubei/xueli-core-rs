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
}

impl MoodState {
    pub fn new() -> Self {
        Self {
            valence: 0.0,
            energy: 0.8,
            arousal: 0.5,
            updated_at: String::new(),
        }
    }
}

impl Default for MoodState {
    fn default() -> Self {
        Self::new()
    }
}

/// 关系亲密度档案
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipProfile {
    /// 用户 ID
    pub user_id: String,
    /// 互动次数
    pub interaction_count: u64,
    /// 亲密度分数 (0.0 - 1.0)
    pub closeness: f64,
    /// 已知喜好
    pub known_preferences: Vec<String>,
    /// 首次互动时间
    pub first_interaction: DateTime<Utc>,
}

impl Default for RelationshipProfile {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            interaction_count: 0,
            closeness: 0.0,
            known_preferences: Vec::new(),
            first_interaction: Utc::now(),
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
