/// 索引协调器 — 管理 BM25 和向量索引的同步
pub struct IndexCoordinator;

impl IndexCoordinator {
    pub fn new() -> Self {
        Self
    }

    /// 重建全部索引
    pub async fn rebuild(&self) -> Result<(), String> {
        // TODO: 实现索引重建
        Ok(())
    }

    /// 增量更新索引
    pub async fn update(&self, _memory_ids: &[String]) -> Result<(), String> {
        // TODO: 实现增量更新
        Ok(())
    }
}

impl Default for IndexCoordinator {
    fn default() -> Self {
        Self::new()
    }
}