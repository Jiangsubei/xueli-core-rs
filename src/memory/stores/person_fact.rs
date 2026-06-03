use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 人物事实
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonFact {
    pub id: String,
    pub user_id: String,
    pub fact_text: String,
    pub category: String,
    pub confidence: f64,
    pub source_conversation_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// SQLite 人物事实存储
pub struct SqlitePersonFactStore {
    db_path: String,
}

impl SqlitePersonFactStore {
    pub fn new(db_path: &str) -> Result<Self, String> {
        Ok(Self {
            db_path: db_path.to_string(),
        })
    }

    pub async fn store(&self, _fact: PersonFact) -> Result<String, String> {
        // TODO: 实现 SQLite 存储
        Ok(String::new())
    }

    pub async fn get_by_user(&self, _user_id: &str) -> Result<Vec<PersonFact>, String> {
        // TODO: 实现按用户查询
        Ok(Vec::new())
    }
}