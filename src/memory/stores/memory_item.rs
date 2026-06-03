use async_trait::async_trait;

use crate::core::types::MemoryItem;
use crate::memory::stores::traits::MemoryStore;

/// SQLite 记忆条目存储
pub struct SqliteMemoryItemStore {
    db_path: String,
}

impl SqliteMemoryItemStore {
    pub fn new(db_path: &str) -> Result<Self, String> {
        Ok(Self {
            db_path: db_path.to_string(),
        })
    }

    /// 初始化数据库表
    pub async fn init(&self) -> Result<(), String> {
        // TODO: 创建表结构
        Ok(())
    }
}

#[async_trait]
impl MemoryStore for SqliteMemoryItemStore {
    async fn store(&self, _item: MemoryItem) -> Result<String, String> {
        // TODO: SQLite 存储实现
        Ok(String::new())
    }

    async fn store_batch(&self, _items: Vec<MemoryItem>) -> Result<Vec<String>, String> {
        // TODO: SQLite 批量存储
        Ok(Vec::new())
    }

    async fn get(&self, _id: &str) -> Result<Option<MemoryItem>, String> {
        // TODO: SQLite 按 ID 查询
        Ok(None)
    }

    async fn get_by_user(&self, _user_id: &str) -> Result<Vec<MemoryItem>, String> {
        // TODO: SQLite 按用户查询
        Ok(Vec::new())
    }

    async fn update(&self, _item: MemoryItem) -> Result<(), String> {
        // TODO: SQLite 更新
        Ok(())
    }

    async fn delete(&self, _id: &str) -> Result<(), String> {
        // TODO: SQLite 删除
        Ok(())
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<MemoryItem>, String> {
        // TODO: SQLite 全文搜索
        Ok(Vec::new())
    }
}