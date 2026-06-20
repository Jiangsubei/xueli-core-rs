use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::prelude::XueliResult;

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
    conn: Arc<Mutex<Connection>>,
}

const INIT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS person_facts (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    fact_text       TEXT NOT NULL,
    category        TEXT NOT NULL DEFAULT 'general',
    confidence      REAL NOT NULL DEFAULT 0.5,
    source_conversation_id TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pf_user_id
    ON person_facts(user_id);

CREATE INDEX IF NOT EXISTS idx_pf_category
    ON person_facts(category);

CREATE INDEX IF NOT EXISTS idx_pf_confidence
    ON person_facts(confidence DESC);
";

fn row_to_person_fact(row: &rusqlite::Row) -> rusqlite::Result<PersonFact> {
    let created_at: String = row.get(6)?;
    let updated_at: String = row.get(7)?;

    Ok(PersonFact {
        id: row.get(0)?,
        user_id: row.get(1)?,
        fact_text: row.get(2)?,
        category: row.get(3)?,
        confidence: row.get(4)?,
        source_conversation_id: row.get(5)?,
        created_at: created_at.parse().unwrap_or_default(),
        updated_at: updated_at.parse().unwrap_or_default(),
    })
}

impl SqlitePersonFactStore {
    pub fn new(db_dir: &Path) -> XueliResult<Self> {
        std::fs::create_dir_all(db_dir).map_err(|e| format!("无法创建目录: {e}"))?;
        let db_path = db_dir.join("person_facts.db");

        let conn =
            Connection::open(&db_path).map_err(|e| format!("无法打开数据库 {db_path:?}: {e}"))?;

        conn.execute_batch("PRAGMA journal_mode=WAL")
            .map_err(|e| format!("PRAGMA 失败: {e}"))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL")
            .map_err(|e| format!("PRAGMA 失败: {e}"))?;
        conn.execute_batch(INIT_SCHEMA)
            .map_err(|e| format!("建表失败: {e}"))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 存储人物事实
    pub async fn store(&self, fact: PersonFact) -> XueliResult<String> {
        let id = fact.id.clone();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<String> {
            let conn = conn.lock().map_err(|e| format!("锁错误: {e}"))?;
            conn.execute(
                "INSERT OR REPLACE INTO person_facts
                 (id, user_id, fact_text, category, confidence, source_conversation_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    fact.id,
                    fact.user_id,
                    fact.fact_text,
                    fact.category,
                    fact.confidence,
                    fact.source_conversation_id,
                    fact.created_at.to_rfc3339(),
                    fact.updated_at.to_rfc3339(),
                ],
            )
            .map_err(|e| format!("插入失败: {e}"))?;
            Ok(id)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    /// 按用户查询人物事实
    pub async fn get_by_user(&self, user_id: &str) -> XueliResult<Vec<PersonFact>> {
        let user_id = user_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<Vec<PersonFact>> {
            let conn = conn.lock().map_err(|e| format!("锁错误: {e}"))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, fact_text, category, confidence, source_conversation_id, created_at, updated_at
                     FROM person_facts WHERE user_id = ?1 ORDER BY confidence DESC",
                )
                .map_err(|e| format!("准备查询失败: {e}"))?;

            let rows = stmt
                .query_map(params![user_id], row_to_person_fact)
                .map_err(|e| format!("查询失败: {e}"))?;

            let mut facts = Vec::new();
            for row in rows {
                facts.push(row.map_err(|e| format!("读取行失败: {e}"))?);
            }
            Ok(facts)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    /// 按分类查询人物事实
    pub async fn get_by_category(
        &self,
        user_id: &str,
        category: &str,
    ) -> XueliResult<Vec<PersonFact>> {
        let user_id = user_id.to_string();
        let category = category.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<Vec<PersonFact>> {
            let conn = conn.lock().map_err(|e| format!("锁错误: {e}"))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, fact_text, category, confidence, source_conversation_id, created_at, updated_at
                     FROM person_facts WHERE user_id = ?1 AND category = ?2 ORDER BY confidence DESC",
                )
                .map_err(|e| format!("准备查询失败: {e}"))?;

            let rows = stmt
                .query_map(params![user_id, category], row_to_person_fact)
                .map_err(|e| format!("查询失败: {e}"))?;

            let mut facts = Vec::new();
            for row in rows {
                facts.push(row.map_err(|e| format!("读取行失败: {e}"))?);
            }
            Ok(facts)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    /// 删除人物事实
    pub async fn delete(&self, id: &str) -> XueliResult<()> {
        let id = id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<()> {
            let conn = conn.lock().map_err(|e| format!("锁错误: {e}"))?;
            conn.execute("DELETE FROM person_facts WHERE id = ?1", params![id])
                .map_err(|e| format!("删除失败: {e}"))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fact(id: &str, user_id: &str, text: &str, cat: &str) -> PersonFact {
        PersonFact {
            id: id.to_string(),
            user_id: user_id.to_string(),
            fact_text: text.to_string(),
            category: cat.to_string(),
            confidence: 0.8,
            source_conversation_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqlitePersonFactStore::new(dir.path()).unwrap();

        store
            .store(make_fact("f1", "u1", "用户是程序员", "occupation"))
            .await
            .unwrap();

        let facts = store.get_by_user("u1").await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact_text, "用户是程序员");
    }

    #[tokio::test]
    async fn test_get_by_category() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqlitePersonFactStore::new(dir.path()).unwrap();

        store
            .store(make_fact("f1", "u1", "喜欢咖啡", "preference"))
            .await
            .unwrap();
        store
            .store(make_fact("f2", "u1", "是程序员", "occupation"))
            .await
            .unwrap();

        let prefs = store.get_by_category("u1", "preference").await.unwrap();
        assert_eq!(prefs.len(), 1);
        assert_eq!(prefs[0].fact_text, "喜欢咖啡");
    }

    #[tokio::test]
    async fn test_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqlitePersonFactStore::new(dir.path()).unwrap();

        store
            .store(make_fact("f1", "u1", "测试", "general"))
            .await
            .unwrap();
        assert_eq!(store.get_by_user("u1").await.unwrap().len(), 1);

        store.delete("f1").await.unwrap();
        assert_eq!(store.get_by_user("u1").await.unwrap().len(), 0);
    }
}
