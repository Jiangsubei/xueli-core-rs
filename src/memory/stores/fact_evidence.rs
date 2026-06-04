use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

use crate::prelude::XueliResult;

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
    conn: Mutex<Connection>,
}

const INIT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS fact_evidences (
    id              TEXT PRIMARY KEY,
    fact_id         TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    message_id      TEXT NOT NULL DEFAULT '',
    evidence_text   TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_fe_fact_id
    ON fact_evidences(fact_id);

CREATE INDEX IF NOT EXISTS idx_fe_conversation
    ON fact_evidences(conversation_id);
";

fn row_to_fact_evidence(row: &rusqlite::Row) -> rusqlite::Result<FactEvidence> {
    let created_at: String = row.get(5)?;

    Ok(FactEvidence {
        id: row.get(0)?,
        fact_id: row.get(1)?,
        conversation_id: row.get(2)?,
        message_id: row.get(3)?,
        evidence_text: row.get(4)?,
        created_at: created_at.parse().unwrap_or_default(),
    })
}

impl SqliteFactEvidenceStore {
    pub fn new(db_dir: &Path) -> XueliResult<Self> {
        std::fs::create_dir_all(db_dir).map_err(|e| format!("无法创建目录: {e}"))?;
        let db_path = db_dir.join("fact_evidence.db");

        let conn =
            Connection::open(&db_path).map_err(|e| format!("无法打开数据库 {db_path:?}: {e}"))?;

        conn.execute_batch("PRAGMA journal_mode=WAL")
            .map_err(|e| format!("PRAGMA 失败: {e}"))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL")
            .map_err(|e| format!("PRAGMA 失败: {e}"))?;
        conn.execute_batch(INIT_SCHEMA)
            .map_err(|e| format!("建表失败: {e}"))?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 存储事实证据
    pub async fn store(&self, evidence: FactEvidence) -> XueliResult<String> {
        let id = evidence.id.clone();
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO fact_evidences
             (id, fact_id, conversation_id, message_id, evidence_text, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                evidence.id,
                evidence.fact_id,
                evidence.conversation_id,
                evidence.message_id,
                evidence.evidence_text,
                evidence.created_at.to_rfc3339(),
            ],
        )
        .map_err(|e| format!("插入失败: {e}"))?;
        Ok(id)
    }

    /// 按事实 ID 获取所有证据
    pub async fn get_by_fact(&self, fact_id: &str) -> XueliResult<Vec<FactEvidence>> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, fact_id, conversation_id, message_id, evidence_text, created_at
                 FROM fact_evidences WHERE fact_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(|e| format!("准备查询失败: {e}"))?;

        let rows = stmt
            .query_map(params![fact_id], row_to_fact_evidence)
            .map_err(|e| format!("查询失败: {e}"))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| format!("读取行失败: {e}"))?);
        }
        Ok(items)
    }

    /// 按对话 ID 获取所有证据
    pub async fn get_by_conversation(
        &self,
        conversation_id: &str,
    ) -> XueliResult<Vec<FactEvidence>> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, fact_id, conversation_id, message_id, evidence_text, created_at
                 FROM fact_evidences WHERE conversation_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(|e| format!("准备查询失败: {e}"))?;

        let rows = stmt
            .query_map(params![conversation_id], row_to_fact_evidence)
            .map_err(|e| format!("查询失败: {e}"))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| format!("读取行失败: {e}"))?);
        }
        Ok(items)
    }

    /// 删除事实的所有证据
    pub async fn delete_by_fact(&self, fact_id: &str) -> XueliResult<()> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        conn.execute(
            "DELETE FROM fact_evidences WHERE fact_id = ?1",
            params![fact_id],
        )
        .map_err(|e| format!("删除失败: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_evidence(id: &str, fact_id: &str, conv_id: &str, text: &str) -> FactEvidence {
        FactEvidence {
            id: id.to_string(),
            fact_id: fact_id.to_string(),
            conversation_id: conv_id.to_string(),
            message_id: "msg1".to_string(),
            evidence_text: text.to_string(),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_store_and_get_by_fact() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteFactEvidenceStore::new(dir.path()).unwrap();

        store
            .store(make_evidence("e1", "f1", "c1", "用户在对话中说喜欢咖啡"))
            .await
            .unwrap();
        store
            .store(make_evidence("e2", "f1", "c2", "用户再次提到咖啡"))
            .await
            .unwrap();
        store
            .store(make_evidence("e3", "f2", "c1", "其他事实"))
            .await
            .unwrap();

        let ev = store.get_by_fact("f1").await.unwrap();
        assert_eq!(ev.len(), 2);
    }

    #[tokio::test]
    async fn test_get_by_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteFactEvidenceStore::new(dir.path()).unwrap();

        store
            .store(make_evidence("e1", "f1", "c1", "证据1"))
            .await
            .unwrap();
        store
            .store(make_evidence("e2", "f2", "c1", "证据2"))
            .await
            .unwrap();

        let ev = store.get_by_conversation("c1").await.unwrap();
        assert_eq!(ev.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_by_fact() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteFactEvidenceStore::new(dir.path()).unwrap();

        store
            .store(make_evidence("e1", "f1", "c1", "证据"))
            .await
            .unwrap();
        assert_eq!(store.get_by_fact("f1").await.unwrap().len(), 1);

        store.delete_by_fact("f1").await.unwrap();
        assert_eq!(store.get_by_fact("f1").await.unwrap().len(), 0);
    }
}
