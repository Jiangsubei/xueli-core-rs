use std::collections::VecDeque;
use std::sync::Mutex;

use crate::emoji::database::{EmojiDB, EmojiDatabase, EmojiEntry, StickerRecord};
use crate::prelude::XueliResult;
use crate::services::vision_client::VisionClient;
use crate::traits::ai_client::AIClient;
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

    /// 获取数据库引用
    pub fn database(&self) -> &EmojiDatabase {
        self.db.database()
    }

    /// 采集表情贴纸入库（SHA256 去重，自动计算 hash）
    ///
    /// 从文件字节、消息 ID、用户 ID、群 ID 直接采集，自动检测格式并计算 SHA256。
    /// 返回贴纸记录（None 表示跳过或失败）。
    pub async fn capture_sticker(
        &self,
        file_bytes: &[u8],
        message_id: &str,
        user_id: &str,
        group_id: &str,
    ) -> XueliResult<Option<StickerRecord>> {
        if file_bytes.is_empty() {
            return Ok(None);
        }

        let sha256 = crypto::sha256_bytes(file_bytes);
        if sha256.is_empty() {
            return Ok(None);
        }

        let file_format = Self::detect_format(file_bytes);
        let file_path = format!("data/emojis/{}.{}", sha256, file_format);

        // 保存文件到磁盘
        if !std::path::Path::new(&file_path).exists() {
            let tmp_path = format!("{}.tmp", file_path);
            if let Some(parent) = std::path::Path::new(&file_path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&tmp_path, file_bytes).is_ok() {
                let _ = std::fs::rename(&tmp_path, &file_path);
            }
        }

        let database = self.db.database();
        let record = database
            .save_sticker_async(
                &sha256,
                &file_path,
                &file_format,
                "",
                message_id,
                user_id,
                group_id,
            )
            .await?;

        // 如果新入库，同步刷新内存缓存
        if let Some(ref rec) = record {
            self.db.refresh_cache();
            let emoji_id = format!("emoji_{}", &rec.file_hash[..16.min(rec.file_hash.len())]);
            self.record_usage(&emoji_id).ok();
        }

        Ok(record)
    }

    /// 检测文件格式（通过魔术字节）
    pub fn detect_format(data: &[u8]) -> String {
        if data.len() >= 3 && data[..3] == [b'G', b'I', b'F'] {
            return "gif".to_string();
        }
        if data.len() >= 4 && data[..4] == [0x89, b'P', b'N', b'G'] {
            return "png".to_string();
        }
        if data.len() >= 2 && data[..2] == [0xFF, 0xD8] {
            return "jpg".to_string();
        }
        "gif".to_string()
    }

    /// 根据对话内容推荐表情（优先按情绪匹配，其次随机）
    pub fn recommend(&self, intent_keywords: &str) -> XueliResult<Option<EmojiEntry>> {
        // 先尝试在内存缓存中按情绪搜索
        let emotion = detect_emotion(intent_keywords);
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
    pub fn is_recently_used(&self, emoji_id: &str) -> bool {
        self.recent_used
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == emoji_id)
    }

    /// 对贴纸进行 VLM 情绪分类（委托给 VisionClient）
    ///
    /// 如果 VisionClient 可用，调用视觉模型对贴纸进行情绪分类，
    /// 将结果写入 description 并注册该贴纸。
    pub async fn classify_sticker<A: AIClient>(
        &self,
        vision_client: Option<&VisionClient<A>>,
        file_hash: &str,
        file_data: &[u8],
    ) -> XueliResult<String> {
        let vc = match vision_client {
            Some(v) => v,
            None => return Err("VisionClient 不可用".into()),
        };

        if !vc.is_available() {
            return Err("VLM 未配置".into());
        }

        use base64::{engine::general_purpose::STANDARD, Engine};
        let image_b64 = STANDARD.encode(file_data);

        let mood = vc.classify_sticker_emotion(&image_b64, "").await?;
        let trimmed = mood.trim().to_string();

        if !trimmed.is_empty() {
            let database = self.db.database();
            database.set_description_async(file_hash, &trimmed).await?;
        }

        let database = self.db.database();
        database.register_async(file_hash).await?;
        self.db.refresh_cache();

        Ok(trimmed)
    }

    /// 表情库大小
    pub fn count(&self) -> usize {
        self.db.count()
    }
}

/// 简单的情绪关键词检测
pub fn detect_emotion(text: &str) -> Option<String> {
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
    use tempfile::TempDir;

    fn make_mgr() -> (EmojiManager, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = EmojiDB::new(dir.path().to_str().unwrap());
        (EmojiManager::new(db), dir)
    }

    #[test]
    fn test_detect_format() {
        // PNG magic
        let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(EmojiManager::detect_format(&png), "png");

        // GIF magic
        let gif = [b'G', b'I', b'F', b'8', b'9', b'a'];
        assert_eq!(EmojiManager::detect_format(&gif), "gif");

        // JPEG magic
        let jpg = [0xFFu8, 0xD8, 0xFF, 0xE0];
        assert_eq!(EmojiManager::detect_format(&jpg), "jpg");

        // Unknown -> defaults to gif
        let unknown = [0x00u8, 0x01, 0x02];
        assert_eq!(EmojiManager::detect_format(&unknown), "gif");
    }

    #[tokio::test]
    async fn test_capture_sticker() {
        let (mgr, _dir) = make_mgr();
        let data = b"test_sticker_data_capture_12345";
        let result = mgr
            .capture_sticker(data, "msg_1", "user_1", "group_1")
            .await
            .unwrap();
        assert!(result.is_some());
        let rec = result.unwrap();
        assert_eq!(rec.user_id, "user_1");
        assert_eq!(rec.message_id, "msg_1");
        assert_eq!(rec.group_id.as_deref(), Some("group_1"));
        assert!(!rec.is_registered);
    }

    #[tokio::test]
    async fn test_capture_sticker_empty_data() {
        let (mgr, _dir) = make_mgr();
        let result = mgr
            .capture_sticker(&[], "msg", "user", "group")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_capture_duplicate() {
        let (mgr, _dir) = make_mgr();
        let data = b"same_data_for_dup";
        let r1 = mgr.capture_sticker(data, "msg1", "u1", "g1").await.unwrap();
        assert!(r1.is_some());
        let r2 = mgr.capture_sticker(data, "msg2", "u2", "g2").await.unwrap();
        // 重复 SHA256 仍返回记录（更新 last_seen_at）
        assert!(r2.is_some());
    }

    #[test]
    fn test_detect_emotion() {
        assert_eq!(detect_emotion("哈哈太搞笑了"), Some("happy".into()));
        assert_eq!(detect_emotion("我好难过"), Some("sad".into()));
        assert_eq!(detect_emotion("晚安！拜拜"), Some("bye".into()));
        assert_eq!(detect_emotion("普通聊天"), None);
    }

    #[test]
    fn test_recommend_and_record_usage() {
        let (mgr, _dir) = make_mgr();
        let data = b"test_recommend";
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rec = rt
            .block_on(mgr.capture_sticker(data, "m", "u", "g"))
            .unwrap()
            .unwrap();

        let emoji_id = format!("emoji_{}", &rec.file_hash[..16.min(rec.file_hash.len())]);
        mgr.record_usage(&emoji_id).unwrap();
        assert!(mgr.is_recently_used(&emoji_id));

        let recommendation = mgr.recommend("哈哈").unwrap();
        // 推荐不应该返回最近用过的
        assert!(recommendation.is_none() || recommendation.unwrap().id != emoji_id);
    }
}
