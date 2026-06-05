use std::collections::VecDeque;
use std::sync::Mutex;

use crate::emoji::database::{EmojiDB, EmojiEntry};
use crate::prelude::XueliResult;
use crate::util::crypto;

/// 表情管理器 — 采集、分类、推荐表情。
///
/// 贴纸类型（sticker）自动入库，SHA256 去重；普通图片不做表情采集。
pub struct EmojiManager {
    db: EmojiDB,
    /// 最近使用过的表情 (FIFO 用于冷却)
    recent_used: Mutex<VecDeque<String>>,
    max_recent: usize,
}

impl EmojiManager {
    pub fn new(db: EmojiDB) -> Self {
        Self {
            db,
            recent_used: Mutex::new(VecDeque::new()),
            max_recent: 20,
        }
    }

    /// 采集表情贴纸入库（SHA256 去重，自动计算 hash）
    pub fn capture_sticker(
        &self,
        name: &str,
        file_data: &[u8],
        file_path: &str,
        tags: &[&str],
        emotion_label: Option<&str>,
    ) -> XueliResult<Option<String>> {
        let sha256 = crypto::sha256_bytes(file_data);
        if sha256.is_empty() {
            return Ok(None);
        }

        let entry_id = format!("emoji_{}", &sha256[..16]);
        let mut entry = EmojiEntry::new(&entry_id, name, file_path);
        entry.sha256 = sha256;
        if !tags.is_empty() {
            entry.tags = tags.iter().map(|s| s.to_string()).collect();
        }
        if let Some(label) = emotion_label {
            entry.emotion_label = Some(label.to_string());
        }
        entry.category = "sticker".to_string();

        let id = self.db.add_emoji(entry)?;
        if id.is_empty() {
            return Ok(None);
        }
        Ok(Some(id))
    }

    /// 根据对话内容推荐表情（优先按情绪匹配，其次随机）
    pub fn recommend(&self, message_text: &str) -> XueliResult<Option<EmojiEntry>> {
        // 简单的情绪关键词检测
        let emotion = detect_emotion(message_text);
        if let Some(em) = &emotion {
            if let Ok(Some(entry)) = self.db.find_by_emotion(em) {
                if !self.is_recently_used(&entry.id) {
                    return Ok(Some(entry));
                }
            }
        }
        self.db.get_random(Some("sticker"))
    }

    /// 记录表情使用（冷却追踪）
    pub fn record_usage(&self, emoji_id: &str) -> XueliResult<()> {
        self.db.increment_usage(emoji_id);

        let mut recent = self.recent_used.lock().unwrap();
        recent.push_back(emoji_id.to_string());
        if recent.len() > self.max_recent {
            recent.pop_front();
        }
        Ok(())
    }

    /// 检查是否为近期已用（避免重复发送）
    fn is_recently_used(&self, emoji_id: &str) -> bool {
        self.recent_used
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == emoji_id)
    }

    /// 表情库大小
    pub fn count(&self) -> usize {
        self.db.count()
    }
}

/// 简单的情绪关键词检测
fn detect_emotion(text: &str) -> Option<String> {
    let text = text.to_lowercase();
    let pairs: &[(&[&str], &str)] = &[
        (&["开心", "哈哈", "笑", "😊", "😄", "高兴"], "happy"),
        (&["难过", "哭", "😢", "伤心", "悲伤"], "sad"),
        (&["生气", "愤怒", "😠", "气"], "angry"),
        (&["惊讶", "😮", "震惊", "天哪"], "surprised"),
        (&["爱", "❤", "喜欢", "想"], "love"),
        (&["晚安", "拜拜", "再见", "👋"], "bye"),
        (&["加油", "赞", "👍", "棒"], "encourage"),
    ];
    for (keywords, emotion) in pairs {
        if keywords.iter().any(|k| text.contains(k)) {
            return Some(emotion.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mgr() -> EmojiManager {
        let dir = tempfile::TempDir::new().unwrap();
        EmojiManager::new(EmojiDB::new(
            dir.path().join("emojis.json").to_str().unwrap(),
        ))
    }

    #[test]
    fn test_capture_sticker() {
        let mgr = make_mgr();
        let data = b"test_sticker_data_12345";
        let result = mgr.capture_sticker(
            "测试贴纸",
            data,
            "path/to/sticker.png",
            &["可爱"],
            Some("happy"),
        );
        assert!(result.unwrap().is_some());
        assert_eq!(mgr.count(), 1);
    }

    #[test]
    fn test_capture_duplicate_skipped() {
        let mgr = make_mgr();
        let data = b"same_data";
        mgr.capture_sticker("A", data, "p1.png", &[], None).unwrap();
        let result = mgr.capture_sticker("B", data, "p2.png", &[], None).unwrap();
        // 重复 SHA256 应跳过
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_emotion() {
        assert_eq!(detect_emotion("哈哈太搞笑了"), Some("happy".into()));
        assert_eq!(detect_emotion("我好难过"), Some("sad".into()));
        assert_eq!(detect_emotion("晚安！拜拜"), Some("bye".into()));
        assert_eq!(detect_emotion("普通聊天"), None);
    }

    #[test]
    fn test_record_usage() {
        let mgr = make_mgr();
        let data = b"test";
        let id = mgr
            .capture_sticker("X", data, "p.png", &[], None)
            .unwrap()
            .unwrap();
        mgr.record_usage(&id).unwrap();
        // 不应重复使用
        assert!(mgr.is_recently_used(&id));
    }
}
