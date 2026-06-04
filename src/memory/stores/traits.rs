use async_trait::async_trait;

use crate::prelude::XueliResult;
use crate::core::types::MemoryItem;

/// 记忆存储 trait — 抽象底层存储实现
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// 存储记忆
    async fn store(&self, item: MemoryItem) -> XueliResult<String>;

    /// 批量存储
    async fn store_batch(&self, items: Vec<MemoryItem>) -> XueliResult<Vec<String>>;

    /// 按 ID 获取
    async fn get(&self, id: &str) -> XueliResult<Option<MemoryItem>>;

    /// 按用户 ID 查询
    async fn get_by_user(&self, user_id: &str) -> XueliResult<Vec<MemoryItem>>;

    /// 更新记忆
    async fn update(&self, item: MemoryItem) -> XueliResult<()>;

    /// 删除记忆
    async fn delete(&self, id: &str) -> XueliResult<()>;

    /// 全文搜索
    async fn search(&self, query: &str, limit: usize) -> XueliResult<Vec<MemoryItem>>;
}
