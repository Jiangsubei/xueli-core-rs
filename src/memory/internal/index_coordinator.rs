use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::memory::retrieval::bm25_index::BM25Index;
use crate::memory::retrieval::vector_index::VectorIndex;
use crate::prelude::XueliResult;

/// 索引协调器 — 管理 BM25 和向量索引的同步重建和增量更新。
pub struct IndexCoordinator {
    /// BM25 全文本索引
    bm25: Arc<RwLock<BM25Index>>,
    /// 字符 n-gram 向量索引
    vector: Arc<RwLock<VectorIndex>>,
    /// 标记为脏的文档 ID 集合
    dirty_ids: Arc<RwLock<HashSet<String>>>,
    /// 是否需要全量重建
    needs_rebuild: Arc<RwLock<bool>>,
}

impl IndexCoordinator {
    pub fn new() -> Self {
        Self {
            bm25: Arc::new(RwLock::new(BM25Index::new(1.2, 0.75))),
            vector: Arc::new(RwLock::new(VectorIndex::new(3, 1024))),
            dirty_ids: Arc::new(RwLock::new(HashSet::new())),
            needs_rebuild: Arc::new(RwLock::new(true)),
        }
    }

    /// 从存储数据全量重建 BM25 + 向量索引
    pub async fn rebuild(&self, documents: &[(String, String)]) -> XueliResult<()> {
        let mut bm25 = self.bm25.write().await;
        let mut vector = self.vector.write().await;

        bm25.clear();
        vector.clear();

        for (doc_id, text) in documents {
            bm25.add(doc_id.clone(), text);
            vector.add(doc_id.clone(), text);
        }

        let mut needs = self.needs_rebuild.write().await;
        *needs = false;
        let mut dirty = self.dirty_ids.write().await;
        dirty.clear();

        tracing::info!("索引重建完成，共 {} 个文档", documents.len());
        Ok(())
    }

    /// 增量更新：从索引中移除旧文档，添加新的
    pub async fn update(&self, added: &[(String, String)], removed: &[String]) -> XueliResult<()> {
        let mut bm25 = self.bm25.write().await;
        let mut vector = self.vector.write().await;

        for doc_id in removed {
            bm25.remove_document(doc_id);
            vector.remove_document(doc_id);
        }
        for (doc_id, text) in added {
            bm25.add(doc_id.clone(), text);
            vector.add(doc_id.clone(), text);
        }

        let mut dirty = self.dirty_ids.write().await;
        for doc_id in removed {
            dirty.remove(doc_id);
        }
        for (doc_id, _) in added {
            dirty.remove(doc_id);
        }

        Ok(())
    }

    /// 标记指定文档为脏（需在下次重建时更新）
    pub async fn mark_dirty(&self, doc_id: &str) {
        let mut dirty = self.dirty_ids.write().await;
        dirty.insert(doc_id.to_string());
    }

    /// 确保索引为最新（如果已标记需要重建则触发）
    pub async fn ensure_fresh(&self, documents: &[(String, String)]) -> XueliResult<()> {
        let needs = *self.needs_rebuild.read().await;
        if needs {
            drop(needs);
            self.rebuild(documents).await?;
        }
        Ok(())
    }

    /// BM25 索引引用（只读）
    pub fn bm25(&self) -> Arc<RwLock<BM25Index>> {
        self.bm25.clone()
    }

    /// 向量索引引用（只读）
    pub fn vector(&self) -> Arc<RwLock<VectorIndex>> {
        self.vector.clone()
    }
}

impl Default for IndexCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rebuild_and_ensure_fresh() {
        let coordinator = IndexCoordinator::new();
        let docs = vec![
            ("d1".into(), "你好世界".into()),
            ("d2".into(), "测试文档".into()),
        ];
        coordinator.rebuild(&docs).await.unwrap();

        {
            let needs = *coordinator.needs_rebuild.read().await;
            assert!(!needs);
        }

        // ensure_fresh 不应再重建
        coordinator.ensure_fresh(&[]).await.unwrap();
    }

    #[tokio::test]
    async fn test_mark_dirty_and_update() {
        let coordinator = IndexCoordinator::new();
        coordinator
            .rebuild(&[("d1".into(), "旧内容".into())])
            .await
            .unwrap();

        coordinator.mark_dirty("d1").await;
        {
            let dirty = coordinator.dirty_ids.read().await;
            assert!(dirty.contains("d1"));
        }

        // 增量更新
        coordinator
            .update(&[("d1".into(), "新内容".into())], &[])
            .await
            .unwrap();

        {
            let dirty = coordinator.dirty_ids.read().await;
            assert!(!dirty.contains("d1"));
        }
    }
}
