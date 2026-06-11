use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::prelude::XueliResult;

/// 分享记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareRecord {
    pub id: String,
    pub user_id: String,
    pub content: String,
    pub share_type: ShareType,
    pub source: String,
    pub sent: bool,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_sent_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub target_user_id: String,
    #[serde(default)]
    pub target_group_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShareType {
    Memory,
    Insight,
    Greeting,
    Question,
}

/// 持久化载荷结构
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SharePayload {
    #[serde(default)]
    items: Vec<ShareRecord>,
    #[serde(default)]
    cooldown_until: String,
}

/// 主动分享存储 — 内存优先，JSON 持久化。
pub struct ProactiveShareStore {
    db_path: String,
    payload: Mutex<SharePayload>,
    max_records: usize,
}

impl ProactiveShareStore {
    pub fn new(db_path: &str) -> Self {
        let store = Self {
            db_path: db_path.to_string(),
            payload: Mutex::new(SharePayload::default()),
            max_records: 500,
        };
        store.load_from_disk();
        store
    }

    fn load_from_disk(&self) {
        if std::path::Path::new(&self.db_path).exists() {
            if let Ok(data) = std::fs::read_to_string(&self.db_path) {
                if let Ok(payload) = serde_json::from_str::<SharePayload>(&data) {
                    if let Ok(mut inner) = self.payload.lock() {
                        *inner = payload;
                    }
                }
            }
        }
    }

    fn save_to_disk(&self) {
        let data_str = {
            let inner = self.payload.lock().unwrap();
            serde_json::to_string_pretty(&*inner).unwrap_or_default()
        };
        let tmp = format!("{}.tmp", self.db_path);
        let _ = std::fs::write(&tmp, &data_str);
        let _ = std::fs::rename(&tmp, &self.db_path);
    }

    /// 添加分享记录
    pub fn add_share(
        &self,
        content: &str,
        source: &str,
        expires_in_hours: f64,
        target_user_id: &str,
        target_group_id: &str,
    ) -> XueliResult<ShareRecord> {
        let now = Utc::now();
        let expires_at = now + chrono::Duration::hours(expires_in_hours.max(1.0) as i64);
        let record = ShareRecord {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: String::new(),
            content: content.to_string(),
            share_type: ShareType::Insight,
            source: source.to_string(),
            sent: false,
            created_at: now,
            expires_at: Some(expires_at),
            last_sent_at: None,
            target_user_id: target_user_id.trim().to_string(),
            target_group_id: target_group_id.trim().to_string(),
        };
        let result = record.clone();
        {
            let mut inner = self.payload.lock().map_err(|e| e.to_string())?;
            inner.items.push(record);
            while inner.items.len() > self.max_records {
                inner.items.remove(0);
            }
        }
        self.save_to_disk();
        Ok(result)
    }

    /// 保存分享记录（兼容旧接口）
    pub fn save(&self, mut record: ShareRecord) -> XueliResult<String> {
        if record.id.is_empty() {
            record.id = uuid::Uuid::new_v4().to_string();
        }
        let id = record.id.clone();
        {
            let mut inner = self.payload.lock().map_err(|e| e.to_string())?;
            inner.items.push(record);
            while inner.items.len() > self.max_records {
                inner.items.remove(0);
            }
        }
        self.save_to_disk();
        Ok(id)
    }

    /// 查询最近的分享记录
    pub fn get_recent(&self, user_id: &str, limit: usize) -> XueliResult<Vec<ShareRecord>> {
        let inner = self.payload.lock().map_err(|e| e.to_string())?;
        let limit = limit.min(inner.items.len());
        Ok(inner
            .items
            .iter()
            .rev()
            .filter(|r| r.user_id == user_id || user_id.is_empty())
            .take(limit)
            .cloned()
            .collect())
    }

    /// 标记分享为已发送（设置 last_sent_at）
    pub fn mark_sent(&self, share_id: &str) -> XueliResult<()> {
        {
            let mut inner = self.payload.lock().map_err(|e| e.to_string())?;
            for record in inner.items.iter_mut() {
                if record.id == share_id {
                    record.sent = true;
                    record.last_sent_at = Some(Utc::now());
                    break;
                }
            }
        }
        self.save_to_disk();
        Ok(())
    }

    /// 统计今天已发送数（按 last_sent_at 日期判断）
    pub fn count_sent_today(&self) -> usize {
        let inner = match self.payload.lock() {
            Ok(i) => i,
            Err(_) => return 0,
        };
        let today = Utc::now().format("%Y-%m-%d").to_string();
        inner
            .items
            .iter()
            .filter(|r| {
                r.last_sent_at
                    .as_ref()
                    .map(|dt| dt.format("%Y-%m-%d").to_string() == today)
                    .unwrap_or(false)
            })
            .count()
    }

    /// 查询待发送分享（简单版本）
    pub fn pending_shares(&self, limit: usize) -> XueliResult<Vec<ShareRecord>> {
        let inner = self.payload.lock().map_err(|e| e.to_string())?;
        let now = Utc::now();
        Ok(inner
            .items
            .iter()
            .rev()
            .filter(|r| {
                !r.sent
                    && r.expires_at
                        .as_ref()
                        .map(|exp| *exp > now)
                        .unwrap_or(true)
            })
            .take(limit)
            .cloned()
            .collect())
    }

    /// 查询待发送分享（带冷却和时间窗口检查）
    pub fn pending_shares_with_cooldown(
        &self,
        limit: usize,
        cooldown_hours: f64,
        time_range_start: &str,
        time_range_end: &str,
    ) -> XueliResult<Vec<ShareRecord>> {
        // 检查时间窗口
        if !Self::within_time_range(time_range_start, time_range_end) {
            return Ok(Vec::new());
        }

        let inner = self.payload.lock().map_err(|e| e.to_string())?;
        let now = Utc::now();
        let cooldown_duration = chrono::Duration::hours(cooldown_hours.max(0.0) as i64);
        let valid: Vec<ShareRecord> = inner
            .items
            .iter()
            .filter(|r| {
                // 未过期
                if let Some(exp) = &r.expires_at {
                    if *exp <= now {
                        return false;
                    }
                }
                // 冷却检查
                if cooldown_hours > 0.0 {
                    if let Some(last_sent) = &r.last_sent_at {
                        if now - *last_sent < cooldown_duration {
                            return false;
                        }
                    }
                }
                true
            })
            .take(limit)
            .cloned()
            .collect();

        // 清理过期记录
        drop(inner);
        self.cleanup_expired();
        Ok(valid)
    }

    /// 设置全局冷却
    pub fn set_global_cooldown(&self, hours: f64) {
        let cooldown_until = Utc::now() + chrono::Duration::hours(hours.max(0.0) as i64);
        {
            let mut inner = self.payload.lock().unwrap();
            inner.cooldown_until = cooldown_until.to_rfc3339();
        }
        self.save_to_disk();
    }

    /// 检查全局冷却是否激活
    pub fn is_global_cooldown_active(&self) -> bool {
        let inner = match self.payload.lock() {
            Ok(i) => i,
            Err(_) => return false,
        };
        if inner.cooldown_until.is_empty() {
            return false;
        }
        match DateTime::parse_from_rfc3339(&inner.cooldown_until) {
            Ok(dt) => Utc::now() < dt.with_timezone(&Utc),
            Err(_) => false,
        }
    }

    /// 清理过期记录
    fn cleanup_expired(&self) {
        let now = Utc::now();
        {
            let mut inner = self.payload.lock().unwrap();
            inner.items.retain(|r| {
                r.expires_at
                    .as_ref()
                    .map(|exp| *exp > now)
                    .unwrap_or(true)
            });
        }
        self.save_to_disk();
    }

    /// 检查当前时间是否在可发送时间窗口内
    fn within_time_range(time_range_start: &str, time_range_end: &str) -> bool {
        let start_parts: Vec<&str> = time_range_start.split(':').collect();
        let end_parts: Vec<&str> = time_range_end.split(':').collect();
        let (start_min, end_min) = match (start_parts.len(), end_parts.len()) {
            (2, 2) => {
                let start_h: i32 = start_parts[0].parse().unwrap_or(9);
                let start_m: i32 = start_parts[1].parse().unwrap_or(0);
                let end_h: i32 = end_parts[0].parse().unwrap_or(22);
                let end_m: i32 = end_parts[1].parse().unwrap_or(0);
                (start_h * 60 + start_m, end_h * 60 + end_m)
            }
            _ => return true,
        };
        let now = chrono::Local::now().time();
        let current_minutes = now.hour() as i32 * 60 + now.minute() as i32;
        if end_min < start_min {
            current_minutes >= start_min || current_minutes <= end_min
        } else {
            start_min <= current_minutes && current_minutes <= end_min
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(id: &str, user_id: &str) -> ShareRecord {
        ShareRecord {
            id: id.into(),
            user_id: user_id.into(),
            content: "测试分享".into(),
            share_type: ShareType::Insight,
            source: "insight".into(),
            sent: false,
            created_at: Utc::now(),
            expires_at: None,
            last_sent_at: None,
            target_user_id: String::new(),
            target_group_id: String::new(),
        }
    }

    #[test]
    fn test_save_and_get_recent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("shares.json");
        let store = ProactiveShareStore::new(path.to_str().unwrap());

        store.save(make_record("s1", "u1")).unwrap();
        store.save(make_record("s2", "u2")).unwrap();

        let recent = store.get_recent("u1", 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, "s1");
    }

    #[test]
    fn test_mark_sent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("shares.json");
        let store = ProactiveShareStore::new(path.to_str().unwrap());

        store.save(make_record("s1", "u1")).unwrap();
        store.mark_sent("s1").unwrap();

        let pending = store.pending_shares(10).unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn test_add_share() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("shares.json");
        let store = ProactiveShareStore::new(path.to_str().unwrap());

        let record = store.add_share("测试内容", "insight", 168.0, "user1", "group1").unwrap();
        assert!(!record.id.is_empty());
        assert_eq!(record.content, "测试内容");
        assert_eq!(record.target_user_id, "user1");
        assert!(record.expires_at.is_some());
    }

    #[test]
    fn test_global_cooldown() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("shares.json");
        let store = ProactiveShareStore::new(path.to_str().unwrap());

        assert!(!store.is_global_cooldown_active());
        store.set_global_cooldown(6.0);
        assert!(store.is_global_cooldown_active());
    }

    #[test]
    fn test_count_sent_today() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("shares.json");
        let store = ProactiveShareStore::new(path.to_str().unwrap());

        store.save(make_record("s1", "u1")).unwrap();
        assert_eq!(store.count_sent_today(), 0);

        store.mark_sent("s1").unwrap();
        assert_eq!(store.count_sent_today(), 1);
    }
}
