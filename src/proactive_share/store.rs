use chrono::{DateTime, Utc};
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
    pub sent: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShareType {
    Memory,
    Insight,
    Greeting,
    Question,
}

/// 主动分享存储 — 内存优先，可选 JSON 持久化。
pub struct ProactiveShareStore {
    db_path: String,
    records: Mutex<Vec<ShareRecord>>,
    max_records: usize,
}

impl ProactiveShareStore {
    pub fn new(db_path: &str) -> Self {
        let store = Self {
            db_path: db_path.to_string(),
            records: Mutex::new(Vec::new()),
            max_records: 500,
        };
        store.load_from_disk();
        store
    }

    fn load_from_disk(&self) {
        if std::path::Path::new(&self.db_path).exists() {
            if let Ok(data) = std::fs::read_to_string(&self.db_path) {
                if let Ok(records) = serde_json::from_str::<Vec<ShareRecord>>(&data) {
                    if let Ok(mut inner) = self.records.lock() {
                        *inner = records;
                    }
                }
            }
        }
    }

    /// 异步保存到磁盘（不持锁）
    fn save_to_disk_async(db_path: String, data: String) {
        let tmp = format!("{}.tmp", db_path);
        let _ = std::fs::write(&tmp, &data);
        let _ = std::fs::rename(&tmp, &db_path);
    }

    /// 保存分享记录并返回 ID
    pub fn save(&self, record: ShareRecord) -> XueliResult<String> {
        let id = record.id.clone();
        let data_str = {
            let mut inner = self.records.lock().map_err(|e| e.to_string())?;
            inner.push(record);
            while inner.len() > self.max_records {
                inner.remove(0);
            }
            serde_json::to_string_pretty(&*inner).unwrap_or_default()
        };
        Self::save_to_disk_async(self.db_path.clone(), data_str);
        Ok(id)
    }

    /// 查询最近的分享记录
    pub fn get_recent(&self, user_id: &str, limit: usize) -> XueliResult<Vec<ShareRecord>> {
        let inner = self.records.lock().map_err(|e| e.to_string())?;
        let limit = limit.min(inner.len());
        Ok(inner
            .iter()
            .rev()
            .filter(|r| r.user_id == user_id || user_id.is_empty())
            .take(limit)
            .cloned()
            .collect())
    }

    /// 标记分享为已发送
    pub fn mark_sent(&self, share_id: &str) -> XueliResult<()> {
        let data_str = {
            let mut inner = self.records.lock().map_err(|e| e.to_string())?;
            for record in inner.iter_mut() {
                if record.id == share_id {
                    record.sent = true;
                    break;
                }
            }
            serde_json::to_string_pretty(&*inner).unwrap_or_default()
        };
        Self::save_to_disk_async(self.db_path.clone(), data_str);
        Ok(())
    }

    /// 统计今天已发送数
    pub fn count_sent_today(&self) -> usize {
        let inner = match self.records.lock() {
            Ok(i) => i,
            Err(_) => return 0,
        };
        let today = Utc::now().format("%Y-%m-%d").to_string();
        inner
            .iter()
            .filter(|r| r.sent && r.created_at.format("%Y-%m-%d").to_string() == today)
            .count()
    }

    /// 查询待发送分享
    pub fn pending_shares(&self, limit: usize) -> XueliResult<Vec<ShareRecord>> {
        let inner = self.records.lock().map_err(|e| e.to_string())?;
        Ok(inner
            .iter()
            .rev()
            .filter(|r| !r.sent)
            .take(limit)
            .cloned()
            .collect())
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
            sent: false,
            created_at: Utc::now(),
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
}
