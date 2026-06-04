use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

use crate::prelude::XueliResult;

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
    conn: Mutex<Connection>,
}

const INIT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS important_memories (
    id              TEXT PRIMARY KEY,
    memory_id       TEXT NOT NULL,
    user_id         TEXT NOT NULL,
    importance_score REAL NOT NULL DEFAULT 0.0,
    reason          TEXT,
    marked_at       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_im_user_score
    ON important_memories(user_id, importance_score DESC);

CREATE INDEX IF NOT EXISTS idx_im_memory_id
    ON important_memories(memory_id);
";

fn row_to_important_memory(row: &rusqlite::Row) -> rusqlite::Result<ImportantMemory> {
    let marked_at: String = row.get(5)?;

    Ok(ImportantMemory {
        id: row.get(0)?,
        memory_id: row.get(1)?,
        user_id: row.get(2)?,
        importance_score: row.get(3)?,
        reason: row.get(4)?,
        marked_at: marked_at.parse().unwrap_or_default(),
    })
}

impl ImportantMemoryStore {
    pub fn new(db_dir: &Path) -> XueliResult<Self> {
        std::fs::create_dir_all(db_dir).map_err(|e| format!("无法创建目录: {e}"))?;
        let db_path = db_dir.join("important.db");

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

    /// 标记重要记忆
    pub async fn mark(&self, entry: ImportantMemory) -> XueliResult<()> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO important_memories
             (id, memory_id, user_id, importance_score, reason, marked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                entry.id,
                entry.memory_id,
                entry.user_id,
                entry.importance_score,
                entry.reason,
                entry.marked_at.to_rfc3339(),
            ],
        )
        .map_err(|e| format!("标记失败: {e}"))?;
        Ok(())
    }

    /// 按用户获取高重要度记忆（按分数降序，限制条数）
    pub async fn get_important(
        &self,
        user_id: &str,
        limit: usize,
    ) -> XueliResult<Vec<ImportantMemory>> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, memory_id, user_id, importance_score, reason, marked_at
                 FROM important_memories
                 WHERE user_id = ?1
                 ORDER BY importance_score DESC
                 LIMIT ?2",
            )
            .map_err(|e| format!("准备查询失败: {e}"))?;

        let rows = stmt
            .query_map(params![user_id, limit as i64], row_to_important_memory)
            .map_err(|e| format!("查询失败: {e}"))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| format!("读取行失败: {e}"))?);
        }
        Ok(items)
    }

    /// 删除标记（memory_id 维度）
    pub async fn unmark(&self, memory_id: &str) -> XueliResult<()> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        conn.execute(
            "DELETE FROM important_memories WHERE memory_id = ?1",
            params![memory_id],
        )
        .map_err(|e| format!("取消标记失败: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_important(id: &str, memory_id: &str, user_id: &str, score: f64) -> ImportantMemory {
        ImportantMemory {
            id: id.to_string(),
            memory_id: memory_id.to_string(),
            user_id: user_id.to_string(),
            importance_score: score,
            reason: Some("用户经常提及".to_string()),
            marked_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_mark_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        store
            .mark(make_important("i1", "mem1", "u1", 0.9))
            .await
            .unwrap();
        store
            .mark(make_important("i2", "mem2", "u1", 0.5))
            .await
            .unwrap();
        store
            .mark(make_important("i3", "mem3", "u2", 0.7))
            .await
            .unwrap();

        let items = store.get_important("u1", 10).await.unwrap();
        assert_eq!(items.len(), 2);
        // 高分在前
        assert_eq!(items[0].importance_score, 0.9);
    }

    #[tokio::test]
    async fn test_get_important_with_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        for i in 0..5 {
            store
                .mark(make_important(
                    &format!("i{}", i),
                    &format!("m{}", i),
                    "u1",
                    0.5 + i as f64 * 0.1,
                ))
                .await
                .unwrap();
        }

        let items = store.get_important("u1", 3).await.unwrap();
        assert_eq!(items.len(), 3);
    }

    #[tokio::test]
    async fn test_unmark() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        store
            .mark(make_important("i1", "mem1", "u1", 0.8))
            .await
            .unwrap();
        assert_eq!(store.get_important("u1", 10).await.unwrap().len(), 1);

        store.unmark("mem1").await.unwrap();
        assert_eq!(store.get_important("u1", 10).await.unwrap().len(), 0);
    }
}
