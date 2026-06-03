use crate::core::types::MemoryItem;

/// 检索协调器 — 统一编排 BM25 + 向量混合检索
pub struct RetrievalCoordinator;

/// 检索结果
#[derive(Debug, Clone)]
pub struct RetrievalResult {
    pub items: Vec<MemoryItem>,
    pub scores: Vec<f64>,
}

impl RetrievalCoordinator {
    pub fn new() -> Self {
        Self
    }

    /// 执行混合检索
    pub async fn retrieve(
        &self,
        _query: &str,
        _user_id: &str,
        _bm25_top_k: usize,
        _vector_top_k: usize,
    ) -> Result<RetrievalResult, String> {
        // TODO: 实现 BM25 + 向量混合检索
        Ok(RetrievalResult {
            items: Vec::new(),
            scores: Vec::new(),
        })
    }
}

impl Default for RetrievalCoordinator {
    fn default() -> Self {
        Self::new()
    }
}