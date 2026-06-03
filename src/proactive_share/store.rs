use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 分享记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareRecord {
    pub id: String,
    pub user_id: String,
    pub content: String,
    pub share_type: ShareType,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShareType {
    Memory,
    Insight,
    Greeting,
    Question,
}

/// 主动分享存储
pub struct ProactiveShareStore {
    db_path: String,
}

impl ProactiveShareStore {
    pub fn new(db_path: &str) -> Self {
        Self {
            db_path: db_path.to_string(),
        }
    }

    pub fn save(&self, _record: ShareRecord) -> Result<String, String> {
        // TODO: SQLite 存储
        Ok(String::new())
    }

    pub fn get_recent(&self, _user_id: &str, _limit: usize) -> Result<Vec<ShareRecord>, String> {
        // TODO: 查询最近分享
        Ok(Vec::new())
    }
}