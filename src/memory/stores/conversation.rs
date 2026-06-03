use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::memory::stores::traits::MemoryStore;
use crate::core::types::MemoryItem;

/// 对话记录（存储在 SQLite 中）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub id: String,
    pub session_id: String,
    pub user_id: String,
    pub sender_name: String,
    pub message_text: String,
    pub is_bot_reply: bool,
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// SQLite 对话历史存储
pub struct SqliteConversationStore {
    db_path: String,
}

impl SqliteConversationStore {
    pub fn new(db_path: &str) -> Result<Self, String> {
        Ok(Self {
            db_path: db_path.to_string(),
        })
    }

    /// 插入对话记录
    pub async fn insert(&self, _record: ConversationRecord) -> Result<String, String> {
        // TODO: 实现 SQLite 插入
        Ok(String::new())
    }

    /// 获取会话的对话历史
    pub async fn get_session_history(
        &self,
        _session_id: &str,
        _limit: usize,
    ) -> Result<Vec<ConversationRecord>, String> {
        // TODO: 实现按会话查询
        Ok(Vec::new())
    }
}