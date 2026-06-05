use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Mutex;

/// 叙事线 — 追踪长期相处中的叙事脉络
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NarrativeThread {
    pub id: String,
    pub user_id: String,
    pub theme: String,
    pub summary: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub events: Vec<NarrativeEvent>,
    /// 对话轮次计数（用于判断是否需要更新）
    pub turn_count_since_last_update: usize,
}

/// 叙事事件
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NarrativeEvent {
    pub timestamp: DateTime<Utc>,
    pub description: String,
    pub significance: f64,
}

/// 叙事服务 — 管理每个用户的叙事线。
pub struct NarrativeService {
    threads: Mutex<HashMap<String, NarrativeThread>>,
    /// 每多少轮对话触发一次叙事更新
    update_interval: usize,
    storage_dir: String,
}

impl NarrativeThread {
    pub fn new(user_id: &str, theme: &str) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: user_id.to_string(),
            theme: theme.to_string(),
            summary: String::new(),
            created_at: now,
            updated_at: now,
            events: Vec::new(),
            turn_count_since_last_update: 0,
        }
    }
}

impl NarrativeService {
    pub fn new(storage_dir: &str) -> Self {
        Self {
            threads: Mutex::new(HashMap::new()),
            update_interval: 50,
            storage_dir: storage_dir.to_string(),
        }
    }

    /// 获取用户的叙事线（不存在则创建）
    pub fn get_thread(&self, user_id: &str) -> NarrativeThread {
        let mut threads = self.threads.lock().unwrap();
        threads
            .entry(user_id.to_string())
            .or_insert_with(|| Self::load_or_create(user_id, &self.storage_dir))
            .clone()
    }

    /// 添加事件到叙事线
    pub fn add_event(&self, user_id: &str, description: &str, significance: f64) {
        let data_str = {
            let mut threads = self.threads.lock().unwrap();
            let thread = threads
                .entry(user_id.to_string())
                .or_insert_with(|| Self::load_or_create(user_id, &self.storage_dir));
            thread.events.push(NarrativeEvent {
                timestamp: Utc::now(),
                description: description.to_string(),
                significance,
            });
            thread.turn_count_since_last_update += 1;
            thread.updated_at = Utc::now();
            // 保留最近 200
            if thread.events.len() > 200 {
                thread.events.drain(..thread.events.len() - 200);
            }
            serde_json::to_string_pretty(thread).ok()
        };
        if let Some(data) = data_str {
            let path = self.thread_path(user_id);
            let tmp = format!("{}.tmp", path);
            let _ = std::fs::write(&tmp, &data);
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// 更新叙事主线摘要
    pub fn update_summary(&self, user_id: &str, summary: &str) {
        let data_str = {
            let mut threads = self.threads.lock().unwrap();
            let thread = threads
                .entry(user_id.to_string())
                .or_insert_with(|| Self::load_or_create(user_id, &self.storage_dir));
            thread.summary = summary.to_string();
            thread.updated_at = Utc::now();
            serde_json::to_string_pretty(thread).ok()
        };
        if let Some(data) = data_str {
            let path = self.thread_path(user_id);
            let tmp = format!("{}.tmp", path);
            let _ = std::fs::write(&tmp, &data);
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// 检查是否应触发叙事更新
    pub fn should_update(&self, user_id: &str) -> bool {
        let thread = self.get_thread(user_id);
        thread.turn_count_since_last_update >= self.update_interval
    }

    /// 标记叙事已更新（重置计数）
    pub fn mark_updated(&self, user_id: &str) {
        let mut threads = self.threads.lock().unwrap();
        if let Some(thread) = threads.get_mut(user_id) {
            thread.turn_count_since_last_update = 0;
        }
    }

    fn thread_path(&self, user_id: &str) -> String {
        format!("{}/{}.json", self.storage_dir, user_id)
    }

    fn load_or_create(user_id: &str, dir: &str) -> NarrativeThread {
        let path = format!("{}/{}.json", dir, user_id);
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(thread) = serde_json::from_str::<NarrativeThread>(&data) {
                return thread;
            }
        }
        NarrativeThread::new(user_id, "default")
    }

    fn save_one(&self, user_id: &str) {
        let data_str = {
            let threads = self.threads.lock().unwrap();
            threads
                .get(user_id)
                .and_then(|t| serde_json::to_string_pretty(t).ok())
        };
        if let Some(data) = data_str {
            let path = self.thread_path(user_id);
            let tmp = format!("{}.tmp", path);
            let _ = std::fs::write(&tmp, &data);
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

impl Default for NarrativeService {
    fn default() -> Self {
        Self::new("data/narratives")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_service() -> NarrativeService {
        let dir = tempfile::TempDir::new().unwrap();
        NarrativeService::new(dir.path().to_str().unwrap())
    }

    #[test]
    fn test_new_thread() {
        let thread = NarrativeThread::new("u1", "学习编程");
        assert_eq!(thread.user_id, "u1");
        assert_eq!(thread.theme, "学习编程");
        assert_eq!(thread.turn_count_since_last_update, 0);
    }

    #[test]
    fn test_add_event() {
        let svc = make_service();
        svc.add_event("u1", "用户开始学习 Rust", 0.6);
        let thread = svc.get_thread("u1");
        assert_eq!(thread.events.len(), 1);
        assert_eq!(thread.turn_count_since_last_update, 1);
    }

    #[test]
    fn test_should_update() {
        let svc = make_service();
        // 新线程不应立即触发更新
        assert!(!svc.should_update("u1"));

        // 手动增加足够多的事件
        for i in 0..60 {
            svc.add_event("u1", &format!("对话轮次 {}", i), 0.3);
        }
        assert!(svc.should_update("u1"));

        svc.mark_updated("u1");
        assert!(!svc.should_update("u1"));
    }
}
