use async_trait::async_trait;

use crate::core::types::MemoryItem;

/// 记忆存储 trait — 抽象底层存储实现
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// 存储记忆
    async fn store(&self, item: MemoryItem) -> Result<String, String>;

    /// 批量存储
    async fn store_batch(&self, items: Vec<MemoryItem>) -> Result<Vec<String>, String>;

    /// 按 ID 获取
    async fn get(&self, id: &str) -> Result<Option<MemoryItem>, String>;

    /// 按用户 ID 查询
    async fn get_by_user(&self, user_id: &str) -> Result<Vec<MemoryItem>, String>;

    /// 更新记忆
    async fn update(&self, item: MemoryItem) -> Result<(), String>;

    /// 删除记忆
    async fn delete(&self, id: &str) -> Result<(), String>;

    /// 全文搜索
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryItem>, String>;
}