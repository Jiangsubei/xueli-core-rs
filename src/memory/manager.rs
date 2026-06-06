use std::sync::Arc;

use crate::core::config::MemoryConfig;
use crate::core::types::{MemoryItem, MemoryPatch};
use crate::memory::stores::traits::MemoryStore;
use crate::prelude::XueliResult;

/// 记忆管理器 — 记忆系统的顶层入口
pub struct MemoryManager {
    config: Arc<MemoryConfig>,
    store: Arc<dyn MemoryStore>,
}

impl MemoryManager {
    pub fn new(config: Arc<MemoryConfig>, store: Arc<dyn MemoryStore>) -> Self {
        Self { config, store }
    }

    /// 存储新记忆
    pub async fn store(&self, item: MemoryItem) -> XueliResult<String> {
        self.store.store(item).await
    }

    /// 批量存储记忆
    pub async fn store_batch(&self, items: Vec<MemoryItem>) -> XueliResult<Vec<String>> {
        self.store.store_batch(items).await
    }

    /// 应用记忆 Patch（增删改）
    pub async fn apply_patch(&self, patch: MemoryPatch) -> XueliResult<()> {
        // 新增
        if !patch.add.is_empty() {
            self.store.store_batch(patch.add).await?;
        }

        // 更新
        for item in patch.update {
            self.store.update(item).await?;
        }

        // 删除
        for id in patch.remove {
            self.store.delete(&id).await?;
        }

        Ok(())
    }

    /// 按用户 ID 获取记忆
    pub async fn get_by_user(&self, user_id: &str) -> XueliResult<Vec<MemoryItem>> {
        self.store.get_by_user(user_id).await
    }

    /// 按 ID 获取单条记忆
    pub async fn get(&self, id: &str) -> XueliResult<Option<MemoryItem>> {
        self.store.get(id).await
    }

    /// 删除记忆
    pub async fn delete(&self, memory_id: &str) -> XueliResult<()> {
        self.store.delete(memory_id).await
    }

    /// 全文搜索记忆
    pub async fn search(&self, query: &str, limit: usize) -> XueliResult<Vec<MemoryItem>> {
        self.store.search(query, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::MemoryType;
    use chrono::Utc;
    use std::sync::Arc;

    use crate::memory::stores::memory_item::SqliteMemoryItemStore;

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

    fn test_config() -> MemoryConfig {
        MemoryConfig {
            enabled: true,
            db_path: ":memory:".to_string(),
            storage_backend: "sqlite".to_string(),
            extraction_min_messages: 5,
            bm25_top_k: 10,
            vector_top_k: 5,
            rerank_top_k: 20,
            dynamic_memory_limit: 8,
            dispute: Default::default(),
            auto_extract: true,
            extract_every_n_turns: 3,
            decay: Default::default(),
            retrieval_weights: Default::default(),
        }
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());
        let mgr = MemoryManager::new(Arc::new(test_config()), store);

        mgr.store(make_item("m1", "u1", "记忆内容")).await.unwrap();
        let result = mgr.get("m1").await.unwrap().unwrap();
        assert_eq!(result.content, "记忆内容");
    }

    #[tokio::test]
    async fn test_apply_patch() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());
        let mgr = MemoryManager::new(Arc::new(test_config()), store);

        let patch = MemoryPatch {
            add: vec![
                make_item("a1", "u1", "新增"),
                make_item("a2", "u1", "新增2"),
            ],
            update: vec![],
            remove: vec![],
        };
        mgr.apply_patch(patch).await.unwrap();
        assert_eq!(mgr.get_by_user("u1").await.unwrap().len(), 2);

        // 更新
        let mut updated = mgr.get("a1").await.unwrap().unwrap();
        updated.content = "已更新".to_string();
        let patch = MemoryPatch {
            add: vec![],
            update: vec![updated],
            remove: vec![],
        };
        mgr.apply_patch(patch).await.unwrap();
        assert_eq!(mgr.get("a1").await.unwrap().unwrap().content, "已更新");

        // 删除
        let patch = MemoryPatch {
            add: vec![],
            update: vec![],
            remove: vec!["a2".to_string()],
        };
        mgr.apply_patch(patch).await.unwrap();
        assert!(mgr.get("a2").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_search() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());
        let mgr = MemoryManager::new(Arc::new(test_config()), store);

        mgr.store(make_item("s1", "u1", "咖啡真好喝"))
            .await
            .unwrap();
        mgr.store(make_item("s2", "u1", "今天天气不错"))
            .await
            .unwrap();

        let results = mgr.search("咖啡", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }
}
