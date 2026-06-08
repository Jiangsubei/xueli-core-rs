use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::prelude::{XueliError, XueliResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportantMemory {
    pub id: String,
    pub user_id: String,
    pub content: String,
    pub priority: i32,
    pub score: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub source: String,
    pub metadata_json: String,
    pub recall_count: i32,
    pub last_recalled_at: Option<DateTime<Utc>>,
}

impl ImportantMemory {
    pub fn new(id: &str, user_id: &str, content: &str) -> Self {
        let now = Utc::now();
        Self {
            id: id.to_string(),
            user_id: user_id.to_string(),
            content: content.to_string(),
            priority: 1,
            score: 0.0,
            created_at: now,
            updated_at: now,
            source: "manual".to_string(),
            metadata_json: "{}".to_string(),
            recall_count: 0,
            last_recalled_at: None,
        }
    }
}

pub struct ImportantMemoryStore {
    conn: Arc<Mutex<Connection>>,
    _db_path: PathBuf,
}

const INIT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS important_memories (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    content         TEXT NOT NULL,
    priority        INTEGER NOT NULL DEFAULT 1,
    score           REAL NOT NULL DEFAULT 0.0,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    source          TEXT NOT NULL DEFAULT '',
    metadata_json   TEXT NOT NULL DEFAULT '{}',
    recall_count    INTEGER NOT NULL DEFAULT 0,
    last_recalled_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_im_user_score
    ON important_memories(user_id, score DESC);

CREATE INDEX IF NOT EXISTS idx_im_user_priority
    ON important_memories(user_id, priority DESC);
";

fn row_to_important_memory(row: &rusqlite::Row) -> rusqlite::Result<ImportantMemory> {
    let created_at: String = row.get(5)?;
    let updated_at: String = row.get(6)?;
    let last_recalled: Option<String> = row.get(10)?;

    Ok(ImportantMemory {
        id: row.get(0)?,
        user_id: row.get(1)?,
        content: row.get(2)?,
        priority: row.get(3)?,
        score: row.get(4)?,
        created_at: created_at.parse().unwrap_or_default(),
        updated_at: updated_at.parse().unwrap_or_default(),
        source: row.get(7)?,
        metadata_json: row.get(8)?,
        recall_count: row.get(9)?,
        last_recalled_at: last_recalled.and_then(|s| s.parse().ok()),
    })
}

fn normalize_text(text: &str) -> String {
    let mut s = text.to_lowercase();
    s.retain(|c| c.is_alphanumeric() || c == ' ' || ('\u{4e00}'..='\u{9fff}').contains(&c));
    s.trim().to_string()
}

fn score_match(query: &str, content: &str) -> f64 {
    let nq = normalize_text(query);
    let nc = normalize_text(content);
    if nq.is_empty() || nc.is_empty() {
        return 0.0;
    }
    if nq == nc {
        return 1.0;
    }
    let substring_score = if nc.contains(&nq) || nq.contains(&nc) {
        let shorter = nq.chars().count().min(nc.chars().count());
        let longer = nq.chars().count().max(nc.chars().count());
        shorter as f64 / longer.max(1) as f64
    } else {
        0.0
    };
    let q_chars: std::collections::HashSet<char> = nq.chars().collect();
    let c_chars: std::collections::HashSet<char> = nc.chars().collect();
    let overlap = q_chars.intersection(&c_chars).count();
    let overlap_score = overlap as f64 / c_chars.len().max(1) as f64;
    substring_score.max(overlap_score)
}

impl ImportantMemoryStore {
    pub fn new(db_dir: &std::path::Path) -> XueliResult<Self> {
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
            conn: Arc::new(Mutex::new(conn)),
            _db_path: db_path,
        })
    }

    pub async fn mark(&self, entry: ImportantMemory) -> XueliResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        conn.execute(
            "INSERT OR REPLACE INTO important_memories
             (id, user_id, content, priority, score, created_at, updated_at, source, metadata_json, recall_count, last_recalled_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                entry.id,
                entry.user_id,
                entry.content,
                entry.priority,
                entry.score,
                entry.created_at.to_rfc3339(),
                entry.updated_at.to_rfc3339(),
                entry.source,
                entry.metadata_json,
                entry.recall_count,
                entry.last_recalled_at.map(|t| t.to_rfc3339()),
            ],
        )
        .map_err(|e| XueliError::Database(format!("标记失败: {e}")))?;
        Ok(())
    }

    pub async fn get_important(
        &self,
        user_id: &str,
        limit: usize,
    ) -> XueliResult<Vec<ImportantMemory>> {
        let user_id = user_id.to_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, content, priority, score, created_at, updated_at, source,
                        metadata_json, recall_count, last_recalled_at
                 FROM important_memories
                 WHERE user_id = ?1
                 ORDER BY score DESC, priority DESC
                 LIMIT ?2",
            )
            .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

        let rows = stmt
            .query_map(params![user_id, limit as i64], row_to_important_memory)
            .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| XueliError::Database(format!("读取行失败: {e}")))?);
        }
        Ok(items)
    }

    pub async fn unmark(&self, memory_id: &str) -> XueliResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        conn.execute(
            "DELETE FROM important_memories WHERE id = ?1",
            params![memory_id],
        )
        .map_err(|e| XueliError::Database(format!("取消标记失败: {e}")))?;
        Ok(())
    }

    pub async fn add_memory(
        &self,
        user_id: &str,
        content: &str,
        source: &str,
        priority: i32,
        metadata_json: Option<&str>,
    ) -> XueliResult<Option<ImportantMemory>> {
        let user_id = user_id.to_string();
        let content = content.trim().to_string();
        if content.is_empty() {
            return Ok(None);
        }
        let source = source.to_string();
        let priority = priority.max(1);
        let metadata_json = metadata_json.unwrap_or("{}").to_string();
        let now = Utc::now();
        let mem_id = format!(
            "imp_{}_{:04x}",
            now.format("%Y%m%d%H%M%S"),
            (content.len() as u16)
        );

        let entry = ImportantMemory {
            id: mem_id.clone(),
            user_id: user_id.clone(),
            content: content.clone(),
            priority,
            score: 0.0,
            created_at: now,
            updated_at: now,
            source: source.clone(),
            metadata_json: metadata_json.clone(),
            recall_count: 0,
            last_recalled_at: None,
        };

        self.mark(entry).await?;
        Ok(Some(ImportantMemory {
            id: mem_id,
            user_id,
            content,
            priority,
            score: 0.0,
            created_at: now,
            updated_at: now,
            source,
            metadata_json,
            recall_count: 0,
            last_recalled_at: None,
        }))
    }

    pub async fn get_memories(
        &self,
        user_id: &str,
        min_priority: i32,
    ) -> XueliResult<Vec<ImportantMemory>> {
        let user_id = user_id.to_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, content, priority, score, created_at, updated_at, source,
                        metadata_json, recall_count, last_recalled_at
                 FROM important_memories
                 WHERE user_id = ?1 AND priority >= ?2
                 ORDER BY priority DESC, created_at DESC",
            )
            .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

        let rows = stmt
            .query_map(params![user_id, min_priority], row_to_important_memory)
            .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| XueliError::Database(format!("读取行失败: {e}")))?);
        }
        Ok(items)
    }

    pub async fn search_memories(
        &self,
        user_id: &str,
        query: &str,
    ) -> XueliResult<Vec<ImportantMemory>> {
        let user_id = user_id.to_string();
        let query = query.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<Vec<ImportantMemory>> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, content, priority, score, created_at, updated_at, source,
                            metadata_json, recall_count, last_recalled_at
                     FROM important_memories
                     WHERE user_id = ?1 AND priority >= 1
                     ORDER BY priority DESC, created_at DESC",
                )
                .map_err(|e| XueliError::Database(format!("准备查询失败: {e}")))?;

            let rows = stmt
                .query_map(params![user_id], row_to_important_memory)
                .map_err(|e| XueliError::Database(format!("查询失败: {e}")))?;

            let mut memories: Vec<ImportantMemory> = Vec::new();
            for row in rows {
                if let Ok(mem) = row {
                    let s = score_match(&query, &mem.content);
                    if s >= 0.25 {
                        memories.push(ImportantMemory { score: s, ..mem });
                    }
                }
            }
            memories.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.priority.cmp(&a.priority))
                    .then_with(|| b.created_at.cmp(&a.created_at))
            });
            Ok(memories.into_iter().take(10).collect())
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    pub async fn replace_memories(
        &self,
        user_id: &str,
        memories: &[ImportantMemory],
    ) -> XueliResult<()> {
        let user_id = user_id.to_string();
        let memories = memories.to_vec();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<()> {
            let conn = conn.lock().map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| XueliError::Database(format!("事务失败: {e}")))?;

            tx.execute(
                "DELETE FROM important_memories WHERE user_id = ?1",
                params![user_id],
            )
            .map_err(|e| XueliError::Database(format!("清理失败: {e}")))?;

            for mem in &memories {
                tx.execute(
                    "INSERT INTO important_memories
                     (id, user_id, content, priority, score, created_at, updated_at, source, metadata_json, recall_count, last_recalled_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        mem.id,
                        mem.user_id,
                        mem.content,
                        mem.priority,
                        mem.score,
                        mem.created_at.to_rfc3339(),
                        mem.updated_at.to_rfc3339(),
                        mem.source,
                        mem.metadata_json,
                        mem.recall_count,
                        mem.last_recalled_at.map(|t| t.to_rfc3339()),
                    ],
                )
                .map_err(|e| XueliError::Database(format!("插入失败: {e}")))?;
            }

            tx.commit()
                .map_err(|e| XueliError::Database(format!("提交事务失败: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    pub async fn clear_memories(&self, user_id: &str) -> XueliResult<usize> {
        let user_id = user_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || -> XueliResult<usize> {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let affected = conn
                .execute(
                    "DELETE FROM important_memories WHERE user_id = ?1",
                    params![user_id],
                )
                .map_err(|e| XueliError::Database(format!("清空失败: {e}")))?;
            Ok(affected)
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    pub async fn mark_recalled(&self, mem_id: &str) -> XueliResult<()> {
        let mem_id = mem_id.to_string();
        let conn = Arc::clone(&self.conn);
        let result = tokio::task::spawn_blocking(move || -> XueliResult<()> {
            let conn = conn.lock().map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let now = Utc::now();
            conn.execute(
                "UPDATE important_memories SET recall_count = recall_count + 1, last_recalled_at = ?1, updated_at = ?2 WHERE id = ?3",
                params![now.to_rfc3339(), now.to_rfc3339(), mem_id],
            )
            .map_err(|e| XueliError::Database(format!("标记召回失败: {e}")))?;
            Ok::<(), XueliError>(())
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?;

        result
    }

    pub async fn delete_memory(&self, mem_id: &str) -> XueliResult<bool> {
        let mem_id = mem_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let affected = conn
                .execute(
                    "DELETE FROM important_memories WHERE id = ?1",
                    params![mem_id],
                )
                .map_err(|e| XueliError::Database(format!("删除失败: {e}")))?;
            Ok(affected > 0)
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }

    pub async fn update_memory(&self, mem_id: &str, new_content: &str) -> XueliResult<bool> {
        let new_content = new_content.trim().to_string();
        if new_content.is_empty() {
            return Ok(false);
        }
        let mem_id = mem_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| XueliError::Database(format!("锁错误: {e}")))?;
            let now = Utc::now();
            let affected = conn
                .execute(
                    "UPDATE important_memories SET content = ?1, updated_at = ?2 WHERE id = ?3",
                    params![new_content, now.to_rfc3339(), mem_id],
                )
                .map_err(|e| XueliError::Database(format!("更新失败: {e}")))?;
            Ok(affected > 0)
        })
        .await
        .map_err(|e| XueliError::Database(format!("spawn_blocking 失败: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_important(id: &str, user_id: &str, content: &str, score: f64) -> ImportantMemory {
        ImportantMemory {
            id: id.to_string(),
            user_id: user_id.to_string(),
            content: content.to_string(),
            priority: 1,
            score,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            source: "manual".to_string(),
            metadata_json: "{}".to_string(),
            recall_count: 0,
            last_recalled_at: None,
        }
    }

    #[tokio::test]
    async fn test_mark_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        store
            .mark(make_important("i1", "u1", "用户喜欢喝咖啡", 0.9))
            .await
            .unwrap();
        store
            .mark(make_important("i2", "u1", "用户不喜欢早起", 0.5))
            .await
            .unwrap();
        store
            .mark(make_important("i3", "u2", "用户住在北京", 0.7))
            .await
            .unwrap();

        let items = store.get_important("u1", 10).await.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].score, 0.9);
    }

    #[tokio::test]
    async fn test_get_important_with_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        for i in 0..5 {
            store
                .mark(make_important(
                    &format!("i{}", i),
                    "u1",
                    &format!("记忆{}", i),
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
            .mark(make_important("i1", "u1", "测试", 0.8))
            .await
            .unwrap();
        assert_eq!(store.get_important("u1", 10).await.unwrap().len(), 1);

        store.unmark("i1").await.unwrap();
        assert_eq!(store.get_important("u1", 10).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_replace_memories() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        let memories = vec![
            make_important("r1", "u1", "A", 0.8),
            make_important("r2", "u1", "B", 0.6),
        ];

        store.replace_memories("u1", &memories).await.unwrap();
        assert_eq!(store.get_important("u1", 10).await.unwrap().len(), 2);

        let new_memories = vec![make_important("r3", "u1", "C", 0.9)];
        store.replace_memories("u1", &new_memories).await.unwrap();

        let all = store.get_important("u1", 10).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "r3");
    }

    #[tokio::test]
    async fn test_clear_memories() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        store
            .mark(make_important("c1", "u1", "记忆1", 0.5))
            .await
            .unwrap();
        store
            .mark(make_important("c2", "u1", "记忆2", 0.3))
            .await
            .unwrap();

        let count = store.clear_memories("u1").await.unwrap();
        assert_eq!(count, 2);
        assert!(store.get_important("u1", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_mark_recalled() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        store
            .mark(make_important("mr1", "u1", "召回测试", 0.7))
            .await
            .unwrap();

        store.mark_recalled("mr1").await.unwrap();

        let memories = store.get_important("u1", 10).await.unwrap();
        assert_eq!(memories[0].recall_count, 1);
        assert!(memories[0].last_recalled_at.is_some());
    }

    #[tokio::test]
    async fn test_search_memories() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        store
            .mark(make_important("s1", "u1", "用户喜欢喝咖啡", 0.5))
            .await
            .unwrap();
        store
            .mark(make_important("s2", "u1", "用户不喜欢早起", 0.5))
            .await
            .unwrap();
        store
            .mark(make_important("s3", "u1", "用户住在北京", 0.7))
            .await
            .unwrap();

        let results = store.search_memories("u1", "咖啡").await.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].content.contains("咖啡"));
    }

    #[tokio::test]
    async fn test_delete_memory() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        store
            .mark(make_important("d1", "u1", "删除测试", 0.5))
            .await
            .unwrap();

        assert!(store.delete_memory("d1").await.unwrap());
        assert!(!store.delete_memory("d1").await.unwrap());
    }

    #[tokio::test]
    async fn test_update_memory() {
        let dir = tempfile::tempdir().unwrap();
        let store = ImportantMemoryStore::new(dir.path()).unwrap();

        store
            .mark(make_important("u1", "u1", "旧内容", 0.5))
            .await
            .unwrap();

        assert!(store.update_memory("u1", "新内容").await.unwrap());

        let memories = store.get_important("u1", 10).await.unwrap();
        assert_eq!(memories[0].content, "新内容");
    }
}
