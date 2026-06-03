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

/// 情绪状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MoodState {
    Happy,
    Neutral,
    Sad,
    Excited,
    Thoughtful,
    Playful,
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