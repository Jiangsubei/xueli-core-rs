use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::prelude::XueliResult;
use crate::core::types::MemoryItem;
use crate::memory::retrieval::bm25_index::BM25Index;
use crate::memory::stores::traits::MemoryStore;

/// 检索协调器 — 统一编排 BM25 + 向量混合检索
pub struct RetrievalCoordinator {
    store: Arc<dyn MemoryStore>,
    bm25: RwLock<BM25Index>,
    /// 文档 ID → MemoryItem ID 映射
    doc_to_memory: RwLock<HashMap<String, String>>,
}

/// 检索结果
#[derive(Debug, Clone)]
pub struct RetrievalResult {
    pub items: Vec<MemoryItem>,
    pub scores: Vec<f64>,
}

impl RetrievalCoordinator {
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self {
            store,
            bm25: RwLock::new(BM25Index::new(1.5, 0.75)),
            doc_to_memory: RwLock::new(HashMap::new()),
        }
    }

    /// 将记忆条目添加到 BM25 索引
    pub async fn index_item(&self, item: &MemoryItem, doc_id: &str) {
        let mut bm25 = self.bm25.write().await;
        bm25.add(doc_id.to_string(), &item.content);

        let mut map = self.doc_to_memory.write().await;
        map.insert(doc_id.to_string(), item.id.clone());
    }

    /// 批量索引记忆条目
    pub async fn index_items(&self, items: &[MemoryItem]) {
        let mut bm25 = self.bm25.write().await;
        let mut map = self.doc_to_memory.write().await;

        for (i, item) in items.iter().enumerate() {
            let doc_id = format!("doc_{}", i);
            bm25.add(doc_id.clone(), &item.content);
            map.insert(doc_id, item.id.clone());
        }
    }

    /// 执行混合检索（当前仅 BM25，向量检索预留）
    pub async fn retrieve(
        &self,
        query: &str,
        user_id: &str,
        bm25_top_k: usize,
        _vector_top_k: usize,
    ) -> XueliResult<RetrievalResult> {
        // BM25 检索
        let bm25 = self.bm25.read().await;
        let bm25_results = bm25.search(query, bm25_top_k);

        let doc_map = self.doc_to_memory.read().await;

        let mut items = Vec::new();
        let mut scores = Vec::new();

        for (doc_id, score) in &bm25_results {
            if let Some(memory_id) = doc_map.get(doc_id) {
                if let Some(item) = self.store.get(memory_id).await? {
                    // 过滤：只返回目标用户或全局记忆
                    if item.user_id == user_id || item.user_id.is_empty() {
                        items.push(item);
                        scores.push(*score);
                    }
                }
            }
        }

        Ok(RetrievalResult { items, scores })
    }

    /// 清空索引（用于重建）
    pub async fn clear_index(&self) {
        let mut bm25 = self.bm25.write().await;
        *bm25 = BM25Index::new(1.5, 0.75);

        let mut map = self.doc_to_memory.write().await;
        map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::MemoryType;
    use crate::memory::stores::memory_item::SqliteMemoryItemStore;
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
    async fn test_index_and_retrieve() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());

        let items = vec![
            make_item("m1", "u1", "用户喜欢喝咖啡，每天早晨都要来一杯"),
            make_item("m2", "u1", "用户住在北京朝阳区"),
            make_item("m3", "u1", "用户是后端开发工程师"),
        ];

        for item in &items {
            store.store(item.clone()).await.unwrap();
        }

        let coordinator = RetrievalCoordinator::new(store.clone());
        coordinator.index_items(&items).await;

        let result = coordinator.retrieve("咖啡", "u1", 5, 0).await.unwrap();
        assert!(!result.items.is_empty());
        assert_eq!(result.items[0].id, "m1");
    }

    #[tokio::test]
    async fn test_retrieve_user_filter() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());

        let items = vec![
            make_item("m1", "u1", "喜欢咖啡"),
            make_item("m2", "u2", "喜欢咖啡"),
        ];

        for item in &items {
            store.store(item.clone()).await.unwrap();
        }

        let coordinator = RetrievalCoordinator::new(store.clone());
        coordinator.index_items(&items).await;

        let result = coordinator.retrieve("咖啡", "u1", 5, 0).await.unwrap();
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].user_id, "u1");
    }
}
