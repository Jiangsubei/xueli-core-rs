use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

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
    pub sha256: String,
    pub emotion_label: Option<String>,
    pub created_at: String,
}

impl EmojiEntry {
    pub fn new(id: &str, name: &str, file_path: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            category: String::new(),
            tags: Vec::new(),
            file_path: file_path.to_string(),
            usage_count: 0,
            sha256: String::new(),
            emotion_label: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// 表情数据库 — 内存 + JSON 文件持久化，按标签和分类检索。
pub struct EmojiDB {
    db_path: String,
    entries: Mutex<Vec<EmojiEntry>>,
    /// SHA256 去重索引
    sha256_index: Mutex<HashMap<String, usize>>,
    /// 被禁用的表情 ID
    banned: Mutex<Vec<String>>,
    max_stickers: usize,
}

impl EmojiDB {
    pub fn new(db_path: &str) -> Self {
        let store = Self {
            db_path: db_path.to_string(),
            entries: Mutex::new(Vec::new()),
            sha256_index: Mutex::new(HashMap::new()),
            banned: Mutex::new(Vec::new()),
            max_stickers: 2000,
        };
        store.load_from_disk();
        store
    }

    fn load_from_disk(&self) {
        if std::path::Path::new(&self.db_path).exists() {
            if let Ok(data) = std::fs::read_to_string(&self.db_path) {
                if let Ok(entries) = serde_json::from_str::<Vec<EmojiEntry>>(&data) {
                    if let Ok(mut inner) = self.entries.lock() {
                        *inner = entries;
                    }
                    // 重建索引
                    self.rebuild_index();
                }
            }
        }
    }

    fn save_to_disk(&self) {
        let data = {
            let entries = self.entries.lock().unwrap();
            serde_json::to_string_pretty(&*entries).unwrap_or_default()
        };
        let tmp = format!("{}.tmp", self.db_path);
        let _ = std::fs::write(&tmp, &data);
        let _ = std::fs::rename(&tmp, &self.db_path);
    }

    fn rebuild_index(&self) {
        if let (Ok(entries), Ok(mut index)) = (self.entries.lock(), self.sha256_index.lock()) {
            index.clear();
            for (i, entry) in entries.iter().enumerate() {
                if !entry.sha256.is_empty() {
                    index.insert(entry.sha256.clone(), i);
                }
            }
        }
    }

    /// 添加表情条目（SHA256 去重）
    pub fn add_emoji(&self, entry: EmojiEntry) -> XueliResult<String> {
        if !entry.sha256.is_empty() {
            let index = self.sha256_index.lock().unwrap();
            if index.contains_key(&entry.sha256) {
                return Ok(String::new());
            }
            drop(index);
        }

        let (exists, emoji_id) = {
            let mut entries = self.entries.lock().unwrap();
            // 检查重复
            for e in entries.iter() {
                if (!entry.sha256.is_empty() && e.sha256 == entry.sha256)
                    || e.file_path == entry.file_path
                {
                    return Ok(e.id.clone());
                }
            }
            let id = entry.id.clone();
            entries.push(entry);

            // 溢出处理：删除最旧的
            while entries.len() > self.max_stickers {
                let old = entries.remove(0);
                if !old.sha256.is_empty() {
                    self.sha256_index.lock().unwrap().remove(&old.sha256);
                }
            }

            (false, id)
        };

        self.rebuild_index();
        self.save_to_disk();
        Ok(emoji_id)
    }

    /// 按标签搜索表情
    pub fn find_by_tags(&self, tags: &[&str]) -> XueliResult<Vec<EmojiEntry>> {
        let entries = self.entries.lock().unwrap();
        let banned = self.banned.lock().unwrap();

        let lower_tags: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();
        let results: Vec<EmojiEntry> = entries
            .iter()
            .filter(|e| {
                !banned.contains(&e.id)
                    && lower_tags
                        .iter()
                        .any(|t| e.tags.iter().any(|et| et.to_lowercase().contains(t)))
            })
            .cloned()
            .collect();

        Ok(results)
    }

    /// 按分类随机获取一个表情
    pub fn get_random(&self, category: Option<&str>) -> XueliResult<Option<EmojiEntry>> {
        let entries = self.entries.lock().unwrap();
        let banned = self.banned.lock().unwrap();
        let mut rng = rand::thread_rng();

        let candidates: Vec<&EmojiEntry> = entries
            .iter()
            .filter(|e| !banned.contains(&e.id) && category.map_or(true, |c| e.category == c))
            .collect();

        Ok(candidates.choose(&mut rng).cloned().cloned())
    }

    /// 捡一个特定情绪标签的表情
    pub fn find_by_emotion(&self, emotion: &str) -> XueliResult<Option<EmojiEntry>> {
        let entries = self.entries.lock().unwrap();
        let mut rng = rand::thread_rng();

        let candidates: Vec<&EmojiEntry> = entries
            .iter()
            .filter(|e| e.emotion_label.as_deref() == Some(emotion))
            .collect();

        Ok(candidates.choose(&mut rng).cloned().cloned())
    }

    /// 增加使用计数
    pub fn increment_usage(&self, emoji_id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            if let Some(entry) = entries.iter_mut().find(|e| e.id == emoji_id) {
                entry.usage_count += 1;
            }
        }
    }

    /// 禁用某个表情
    pub fn ban(&self, emoji_id: &str) {
        self.banned.lock().unwrap().push(emoji_id.to_string());
    }

    /// 表情总数
    pub fn count(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db() -> EmojiDB {
        let dir = tempfile::TempDir::new().unwrap();
        EmojiDB::new(dir.path().join("emojis.json").to_str().unwrap())
    }

    #[test]
    fn test_add_and_find_by_tags() {
        let db = make_db();
        let mut e = EmojiEntry::new("e1", "笑脸", "path/img.png");
        e.sha256 = "abc123".into();
        e.tags = vec!["开心".into(), "问候".into()];
        db.add_emoji(e).unwrap();

        let found = db.find_by_tags(&["开心"]).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "笑脸");
    }

    #[test]
    fn test_sha256_dedup() {
        let db = make_db();
        let mut e1 = EmojiEntry::new("e1", "A", "p1.png");
        e1.sha256 = "same_hash".into();
        db.add_emoji(e1).unwrap();

        let mut e2 = EmojiEntry::new("e2", "B", "p2.png");
        e2.sha256 = "same_hash".into();
        let id = db.add_emoji(e2).unwrap();
        // 应返回空（重复跳过）
        assert!(id.is_empty());
    }

    #[test]
    fn test_get_random() {
        let db = make_db();
        let mut e = EmojiEntry::new("e1", "笑", "p1.png");
        e.sha256 = "h1".into();
        e.category = "greeting".into();
        db.add_emoji(e).unwrap();

        let result = db.get_random(Some("greeting")).unwrap();
        assert!(result.is_some());
    }
}
