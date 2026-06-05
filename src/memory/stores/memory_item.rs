use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

use crate::core::types::{MemoryItem, MemoryType};
use crate::memory::stores::traits::MemoryStore;
use crate::prelude::XueliResult;

/// SQLite 记忆条目存储
pub struct SqliteMemoryItemStore {
    conn: Mutex<Connection>,
}

const INIT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS memory_items (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    content         TEXT NOT NULL,
    memory_type     TEXT NOT NULL,
    importance      REAL NOT NULL DEFAULT 0.0,
    created_at      TEXT NOT NULL,
    last_accessed_at TEXT NOT NULL,
    access_count    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_mi_user_id
    ON memory_items(user_id);

CREATE INDEX IF NOT EXISTS idx_mi_type
    ON memory_items(memory_type);

CREATE INDEX IF NOT EXISTS idx_mi_importance
    ON memory_items(importance DESC);
";

fn memory_type_to_str(mt: &MemoryType) -> &'static str {
    match mt {
        MemoryType::Fact => "fact",
        MemoryType::Preference => "preference",
        MemoryType::Event => "event",
        MemoryType::Opinion => "opinion",
        MemoryType::Relationship => "relationship",
    }
}

fn str_to_memory_type(s: &str) -> MemoryType {
    match s {
        "fact" => MemoryType::Fact,
        "preference" => MemoryType::Preference,
        "event" => MemoryType::Event,
        "opinion" => MemoryType::Opinion,
        "relationship" => MemoryType::Relationship,
        _ => MemoryType::Fact,
    }
}

fn row_to_memory_item(row: &rusqlite::Row) -> rusqlite::Result<MemoryItem> {
    let created_at: String = row.get(5)?;
    let last_accessed: String = row.get(6)?;
    let mem_type: String = row.get(3)?;

    Ok(MemoryItem {
        id: row.get(0)?,
        user_id: row.get(1)?,
        content: row.get(2)?,
        memory_type: str_to_memory_type(&mem_type),
        importance: row.get(4)?,
        created_at: created_at.parse().unwrap_or_default(),
        last_accessed_at: last_accessed.parse().unwrap_or_default(),
        access_count: row.get(7)?,
    })
}

impl SqliteMemoryItemStore {
    pub fn new(db_dir: &Path) -> XueliResult<Self> {
        std::fs::create_dir_all(db_dir).map_err(|e| format!("无法创建目录: {e}"))?;
        let db_path = db_dir.join("memory.db");

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
}

#[async_trait]
impl MemoryStore for SqliteMemoryItemStore {
    async fn store(&self, item: MemoryItem) -> XueliResult<String> {
        let id = item.id.clone();
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;

        conn.execute(
            "INSERT OR REPLACE INTO memory_items
             (id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                item.id,
                item.user_id,
                item.content,
                memory_type_to_str(&item.memory_type),
                item.importance,
                item.created_at.to_rfc3339(),
                item.last_accessed_at.to_rfc3339(),
                item.access_count as i64,
            ],
        )
        .map_err(|e| format!("插入失败: {e}"))?;

        Ok(id)
    }

    async fn store_batch(&self, items: Vec<MemoryItem>) -> XueliResult<Vec<String>> {
        let mut ids = Vec::with_capacity(items.len());
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("事务失败: {e}"))?;

        for item in &items {
            ids.push(item.id.clone());
            tx.execute(
                "INSERT OR REPLACE INTO memory_items
                 (id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    item.id,
                    item.user_id,
                    item.content,
                    memory_type_to_str(&item.memory_type),
                    item.importance,
                    item.created_at.to_rfc3339(),
                    item.last_accessed_at.to_rfc3339(),
                    item.access_count as i64,
                ],
            )
            .map_err(|e| format!("批量插入失败: {e}"))?;
        }

        tx.commit().map_err(|e| format!("提交事务失败: {e}"))?;
        Ok(ids)
    }

    async fn get(&self, id: &str) -> XueliResult<Option<MemoryItem>> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count
                 FROM memory_items WHERE id = ?1",
            )
            .map_err(|e| format!("准备查询失败: {e}"))?;

        let result = stmt
            .query_row(params![id], row_to_memory_item)
            .optional()
            .map_err(|e| format!("查询失败: {e}"))?;

        Ok(result)
    }

    async fn get_by_user(&self, user_id: &str) -> XueliResult<Vec<MemoryItem>> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count
                 FROM memory_items WHERE user_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(|e| format!("准备查询失败: {e}"))?;

        let rows = stmt
            .query_map(params![user_id], row_to_memory_item)
            .map_err(|e| format!("查询失败: {e}"))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| format!("读取行失败: {e}"))?);
        }
        Ok(items)
    }

    async fn update(&self, item: MemoryItem) -> XueliResult<()> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let affected = conn
            .execute(
                "UPDATE memory_items SET
                    content = ?2,
                    memory_type = ?3,
                    importance = ?4,
                    last_accessed_at = ?5,
                    access_count = ?6
                 WHERE id = ?1",
                params![
                    item.id,
                    item.content,
                    memory_type_to_str(&item.memory_type),
                    item.importance,
                    item.last_accessed_at.to_rfc3339(),
                    item.access_count as i64,
                ],
            )
            .map_err(|e| format!("更新失败: {e}"))?;

        if affected == 0 {
            return Err(format!("未找到记忆: {}", item.id).into());
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> XueliResult<()> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        conn.execute("DELETE FROM memory_items WHERE id = ?1", params![id])
            .map_err(|e| format!("删除失败: {e}"))?;
        Ok(())
    }

    async fn search(&self, query: &str, limit: usize) -> XueliResult<Vec<MemoryItem>> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let pattern = format!("%{}%", query);
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, content, memory_type, importance, created_at, last_accessed_at, access_count
                 FROM memory_items WHERE content LIKE ?1 ORDER BY importance DESC LIMIT ?2",
            )
            .map_err(|e| format!("准备查询失败: {e}"))?;

        let rows = stmt
            .query_map(params![pattern, limit as i64], row_to_memory_item)
            .map_err(|e| format!("搜索失败: {e}"))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| format!("读取行失败: {e}"))?);
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_item(id: &str, user_id: &str, content: &str) -> MemoryItem {
        MemoryItem {
            id: id.to_string(),
            user_id: user_id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Fact,
            importance: 0.5,
            created_at: Utc::now(),
            last_accessed_at: Utc::now(),
            access_count: 0,
        }
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        let item = make_item("mem1", "user1", "用户喜欢喝咖啡");
        let id = store.store(item).await.unwrap();
        assert_eq!(id, "mem1");

        let fetched = store.get("mem1").await.unwrap().unwrap();
        assert_eq!(fetched.content, "用户喜欢喝咖啡");
        assert_eq!(fetched.user_id, "user1");
    }

    #[tokio::test]
    async fn test_get_by_user() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        store.store(make_item("mem1", "u1", "事实A")).await.unwrap();
        store.store(make_item("mem2", "u2", "事实B")).await.unwrap();
        store.store(make_item("mem3", "u1", "事实C")).await.unwrap();

        let items = store.get_by_user("u1").await.unwrap();
        assert_eq!(items.len(), 2);
    }

    #[tokio::test]
    async fn test_search() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        store
            .store(make_item("m1", "u1", "喜欢吃辣的食物"))
            .await
            .unwrap();
        store
            .store(make_item("m2", "u1", "喜欢看电影"))
            .await
            .unwrap();

        let results = store.search("食物", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("食物"));
    }

    #[tokio::test]
    async fn test_update_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        store
            .store(make_item("m1", "u1", "原始内容"))
            .await
            .unwrap();

        let mut updated = store.get("m1").await.unwrap().unwrap();
        updated.content = "更新后内容".to_string();
        store.update(updated).await.unwrap();

        let fetched = store.get("m1").await.unwrap().unwrap();
        assert_eq!(fetched.content, "更新后内容");

        store.delete("m1").await.unwrap();
        assert!(store.get("m1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_store_batch() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteMemoryItemStore::new(dir.path()).unwrap();

        let items = vec![make_item("b1", "u1", "A"), make_item("b2", "u1", "B")];
        let ids = store.store_batch(items).await.unwrap();
        assert_eq!(ids.len(), 2);

        let all = store.get_by_user("u1").await.unwrap();
        assert_eq!(all.len(), 2);
    }
}
