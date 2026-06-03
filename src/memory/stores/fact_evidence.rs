use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 事实证据 — 关联事实与其来源消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactEvidence {
    pub id: String,
    pub fact_id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub evidence_text: String,
    pub created_at: DateTime<Utc>,
}

/// SQLite 事实证据存储
pub struct SqliteFactEvidenceStore {
    db_path: String,
}

impl SqliteFactEvidenceStore {
    pub fn new(db_path: &str) -> Result<Self, String> {
        Ok(Self {
            db_path: db_path.to_string(),
        })
    }

    pub async fn store(&self, _evidence: FactEvidence) -> Result<String, String> {
        // TODO: 实现存储
        Ok(String::new())
    }
}