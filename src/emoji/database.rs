use serde::{Deserialize, Serialize};

use crate::prelude::XueliResult;

/// 表情条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmojiEntry {
    pub id: String,
    pub name: String,
    pub category: String,
    pub tags: Vec<String>,
    pub file_path: String,
    pub usage_count: u64,
}

/// 表情数据库
pub struct EmojiDB {
    db_path: String,
}

impl EmojiDB {
    pub fn new(db_path: &str) -> Self {
        Self {
            db_path: db_path.to_string(),
        }
    }

    pub fn add_emoji(&self, _entry: EmojiEntry) -> XueliResult<String> {
        // TODO: SQLite 存储
        Ok(String::new())
    }

    pub fn find_by_tags(&self, _tags: &[&str]) -> XueliResult<Vec<EmojiEntry>> {
        // TODO: 按标签搜索
        Ok(Vec::new())
    }

    pub fn get_random(&self, _category: Option<&str>) -> XueliResult<Option<EmojiEntry>> {
        // TODO: 随机获取
        Ok(None)
    }
}
