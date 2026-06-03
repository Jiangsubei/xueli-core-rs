use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 重要记忆标记
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportantMemory {
    pub id: String,
    pub memory_id: String,
    pub user_id: String,
    pub importance_score: f64,
    pub reason: Option<String>,
    pub marked_at: DateTime<Utc>,
}

/// SQLite 重要记忆存储
pub struct ImportantMemoryStore {
    db_path: String,
}

impl ImportantMemoryStore {
    pub fn new(db_path: &str) -> Result<Self, String> {
        Ok(Self {
            db_path: db_path.to_string(),
        })
    }

    pub async fn mark(&self, _entry: ImportantMemory) -> Result<(), String> {
        // TODO: 实现标记
        Ok(())
    }

    pub async fn get_important(&self, _user_id: &str, _limit: usize) -> Result<Vec<ImportantMemory>, String> {
        // TODO: 获取高重要度记忆
        Ok(Vec::new())
    }
}