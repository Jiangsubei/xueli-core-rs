use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use chrono::Timelike;
use parking_lot::Mutex;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use crate::core::errors::XueliError;
use crate::emoji::database::{EmojiDB, EmojiEntry, StickerRecord};
use crate::prelude::{AIClient, PromptTemplateLoader, XueliResult};
use crate::services::vision_client::VisionClient;
use crate::util::crypto;

/// 表情任务管理器 — 管理异步任务的生命周期
pub struct EmojiTaskManager {
    tasks: Arc<AsyncMutex<Vec<JoinHandle<()>>>>,
}

impl EmojiTaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(AsyncMutex::new(Vec::new())),
        }
    }

    pub fn create_task<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(future);
        if let Ok(mut tasks) = self.tasks.try_lock() {
            tasks.retain(|h| !h.is_finished());
            tasks.push(handle);
        }
    }

    pub async fn cancel_all(&self) {
        let mut tasks = self.tasks.lock().await;
        let pending: Vec<_> = tasks.drain(..).filter(|h| !h.is_finished()).collect();
        for handle in pending {
            handle.abort();
        }
    }

    pub async fn count(&self) -> usize {
        self.tasks
            .lock()
            .await
            .iter()
            .filter(|h| !h.is_finished())
            .count()
    }
}

pub struct EmojiManager<A: AIClient, L: PromptTemplateLoader> {
    db: EmojiDB,
    recent_used: StdMutex<VecDeque<String>>,
    max_recent: usize,
    vision_client: Option<Arc<VisionClient<A, L>>>,
    classification_handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    activity_flag: Mutex<Instant>,
    classification_enabled: bool,
    idle_seconds: f64,
    classification_interval_seconds: f64,
    classification_windows: Vec<String>,
    emotion_labels: Vec<String>,
    enabled: bool,
    capture_enabled: bool,
    initialized: bool,
    task_manager: EmojiTaskManager,
}

impl<A: AIClient + 'static, L: PromptTemplateLoader + 'static> EmojiManager<A, L> {
    pub fn new(db: EmojiDB) -> Self {
        Self {
            db,
            recent_used: StdMutex::new(VecDeque::new()),
            max_recent: 20,
            vision_client: None,
            classification_handle: parking_lot::Mutex::new(None),
            activity_flag: Mutex::new(Instant::now()),
            classification_enabled: false,
            idle_seconds: 45.0,
            classification_interval_seconds: 30.0,
            classification_windows: Vec::new(),
            emotion_labels: Vec::new(),
            enabled: true,
            capture_enabled: true,
            initialized: false,
            task_manager: EmojiTaskManager::new(),
        }
    }

    /// 初始化表情管理器，同步指标并启动后台分类工作线程
    pub async fn initialize(&mut self) {
        if !self.enabled || self.initialized {
            return;
        }
        self.initialized = true;
        self.sync_metrics();
        self.start_idle_classification_loop_from_init();
    }

    fn start_idle_classification_loop_from_init(&self) {
        if !self.classification_enabled {
            return;
        }
        if self.vision_client.is_none() {
            return;
        }
        // Note: start_idle_classification_loop requires Arc<Self>, so we skip here
        // The caller should use start_idle_classification_loop on an Arc<Self>
    }

    /// 同步指标到数据库统计
    pub fn sync_metrics(&self) {
        // Stats are read from the database directly when needed
        // This method exists for API compatibility with Python version
    }

    pub fn with_vision_client(mut self, vc: Arc<VisionClient<A, L>>) -> Self {
        self.vision_client = Some(vc);
        self
    }

    pub fn with_classification_config(
        mut self,
        enabled: bool,
        idle_seconds: f64,
        interval_seconds: f64,
        windows: Vec<String>,
        emotion_labels: Vec<String>,
    ) -> Self {
        self.classification_enabled = enabled;
        self.idle_seconds = idle_seconds;
        self.classification_interval_seconds = interval_seconds;
        self.classification_windows = windows;
        self.emotion_labels = emotion_labels;
        self
    }

    pub fn database(&self) -> &crate::emoji::database::EmojiDatabase {
        self.db.database()
    }

    pub async fn capture_sticker(
        &self,
        file_bytes: &[u8],
        message_id: &str,
        user_id: &str,
        group_id: &str,
    ) -> XueliResult<Option<StickerRecord>> {
        if !self.enabled || !self.capture_enabled {
            return Ok(None);
        }
        if file_bytes.is_empty() {
            return Ok(None);
        }

        let sha256 = crypto::sha256_bytes(file_bytes);
        if sha256.is_empty() {
            return Ok(None);
        }

        let file_format = Self::detect_format(file_bytes);
        let file_path = format!("data/emojis/{}.{}", sha256, file_format);

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

        if let Some(ref rec) = record {
            self.db.refresh_cache();
            let emoji_id = format!("emoji_{}", &rec.file_hash[..16.min(rec.file_hash.len())]);
            self.record_usage(&emoji_id).ok();
        }

        Ok(record)
    }

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

    pub fn recommend(&self, intent_keywords: &str) -> XueliResult<Option<EmojiEntry>> {
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

    pub fn record_usage(&self, emoji_id: &str) -> XueliResult<()> {
        self.db.increment_usage(emoji_id);

        let mut recent = self.recent_used.lock().unwrap();
        recent.push_back(emoji_id.to_string());
        if recent.len() > self.max_recent {
            recent.pop_front();
        }
        Ok(())
    }

    pub fn is_recently_used(&self, emoji_id: &str) -> bool {
        self.recent_used
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == emoji_id)
    }

    pub async fn classify_sticker(&self, file_hash: &str, file_data: &[u8]) -> XueliResult<String> {
        let vc = self
            .vision_client
            .as_ref()
            .ok_or_else(|| XueliError::Internal("VisionClient 不可用".into()))?;

        if !vc.is_available() {
            return Err(XueliError::Internal("VLM 未配置".into()));
        }

        use base64::{engine::general_purpose::STANDARD, Engine};
        let image_b64 = STANDARD.encode(file_data);

        let empty_tones: Vec<String> = Vec::new();
        let emotion_result = vc
            .classify_sticker_emotion(&image_b64, &self.emotion_labels, &empty_tones)
            .await?;
        let trimmed = emotion_result.primary_emotion.trim().to_string();

        if !trimmed.is_empty() {
            let database = self.db.database();
            database.set_description_async(file_hash, &trimmed).await?;
        }

        let database = self.db.database();
        database.register_async(file_hash).await?;
        self.db.refresh_cache();

        Ok(trimmed)
    }

    pub fn count(&self) -> usize {
        self.db.count()
    }

    pub fn start_idle_classification_loop(self: &Arc<Self>) {
        if !self.classification_enabled {
            return;
        }
        if self.vision_client.is_none() {
            tracing::warn!("[表情管理] 未配置 VisionClient，无法启动空闲分类循环");
            return;
        }

        {
            let mut guard = self.classification_handle.lock();
            if guard.is_some() {
                return;
            }
            let this = Arc::clone(self);
            let h = tokio::spawn(async move {
                this.run_classification_loop().await;
            });
            *guard = Some(h);
        }
    }

    pub fn stop_idle_classification_loop(&self) {
        let mut guard = self.classification_handle.lock();
        if let Some(handle) = guard.take() {
            handle.abort();
        }
    }

    pub fn record_activity(&self) {
        if !self.enabled {
            return;
        }
        *self.activity_flag.lock() = Instant::now();
    }

    /// 处理检测结果回调，启用时同步指标
    pub async fn process_detection_result(&self) {
        if self.enabled {
            self.sync_metrics();
        }
    }

    async fn run_classification_loop(&self) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs_f64(self.next_wait_seconds())) => {
                    if !self.can_run_classification_now() {
                        continue;
                    }
                }
            }

            let database = self.db.database();
            let pending = match database.list_pending_async().await {
                Ok(p) => p,
                Err(_) => {
                    tokio::time::sleep(tokio::time::Duration::from_secs_f64(
                        self.classification_interval_seconds.max(1.0),
                    ))
                    .await;
                    continue;
                }
            };

            if pending.is_empty() {
                return;
            }

            for record in pending {
                self.classify_one(&record).await;
            }
        }
    }

    async fn classify_one(&self, record: &StickerRecord) {
        let file_path = record.file_path.clone();
        if file_path.is_empty() {
            return;
        }

        let file_data = match tokio::fs::read(&file_path).await {
            Ok(d) => d,
            Err(_) => return,
        };

        let database = self.db.database();
        if database
            .set_emotion_status_async(&record.file_hash, "processing")
            .await
            .is_err()
        {
            return;
        }

        if self
            .classify_sticker(&record.file_hash, &file_data)
            .await
            .is_err()
        {
            tracing::warn!("[表情管理] 表情包分类失败");
        }
    }

    fn can_run_classification_now(&self) -> bool {
        if !self.classification_enabled {
            return false;
        }
        let elapsed = self.activity_flag.lock().elapsed().as_secs_f64();
        if elapsed < self.idle_seconds {
            return false;
        }
        self.is_within_classification_window()
    }

    fn next_wait_seconds(&self) -> f64 {
        let elapsed = self.activity_flag.lock().elapsed().as_secs_f64();
        let idle_remaining = (self.idle_seconds - elapsed).max(0.0);
        if idle_remaining > 0.0 {
            return idle_remaining.min(1.0).max(0.05);
        }
        if !self.is_within_classification_window() {
            return self.classification_interval_seconds.min(5.0).max(0.1);
        }
        self.classification_interval_seconds.max(0.05)
    }

    fn is_within_classification_window(&self) -> bool {
        if self.classification_windows.is_empty() {
            return true;
        }
        let now = chrono::Local::now().time();
        let current_minutes = now.hour() as i32 * 60 + now.minute() as i32;
        for (start_min, end_min) in self.parsed_windows() {
            if start_min <= end_min {
                if start_min <= current_minutes && current_minutes < end_min {
                    return true;
                }
            } else if current_minutes >= start_min || current_minutes < end_min {
                return true;
            }
        }
        false
    }

    fn parsed_windows(&self) -> Vec<(i32, i32)> {
        let mut parsed = Vec::new();
        for window in &self.classification_windows {
            let parts: Vec<&str> = window.split('-').collect();
            if parts.len() != 2 {
                continue;
            }
            if let (Ok(s), Ok(e)) = (
                Self::parse_clock_minutes(parts[0]),
                Self::parse_clock_minutes(parts[1]),
            ) {
                parsed.push((s, e));
            }
        }
        parsed
    }

    fn parse_clock_minutes(text: &str) -> Result<i32, ()> {
        let parts: Vec<&str> = text.trim().split(':').collect();
        if parts.len() != 2 {
            return Err(());
        }
        let hours: i32 = parts[0].parse().map_err(|_| ())?;
        let mins: i32 = parts[1].parse().map_err(|_| ())?;
        Ok(hours * 60 + mins)
    }

    pub fn has_pending_stickers(&self) -> bool {
        self.db
            .database()
            .get_stats()
            .map(|s| s.pending > 0)
            .unwrap_or(false)
    }
}

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
    use crate::services::ai_client::DefaultAIClient;
    use crate::services::prompt_loader::NoopPromptTemplateLoader;
    use tempfile::TempDir;

    fn make_mgr() -> (
        EmojiManager<DefaultAIClient, NoopPromptTemplateLoader>,
        TempDir,
    ) {
        let dir = TempDir::new().unwrap();
        let db = EmojiDB::new(dir.path().to_str().unwrap());
        (EmojiManager::new(db), dir)
    }

    #[test]
    fn test_detect_format() {
        let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(
            EmojiManager::<DefaultAIClient, NoopPromptTemplateLoader>::detect_format(&png),
            "png"
        );

        let gif = [b'G', b'I', b'F', b'8', b'9', b'a'];
        assert_eq!(
            EmojiManager::<DefaultAIClient, NoopPromptTemplateLoader>::detect_format(&gif),
            "gif"
        );

        let jpg = [0xFFu8, 0xD8, 0xFF, 0xE0];
        assert_eq!(
            EmojiManager::<DefaultAIClient, NoopPromptTemplateLoader>::detect_format(&jpg),
            "jpg"
        );

        let unknown = [0x00u8, 0x01, 0x02];
        assert_eq!(
            EmojiManager::<DefaultAIClient, NoopPromptTemplateLoader>::detect_format(&unknown),
            "gif"
        );
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
        assert!(recommendation.is_none() || recommendation.unwrap().id != emoji_id);
    }
}
