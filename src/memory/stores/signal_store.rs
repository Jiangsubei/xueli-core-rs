use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use crate::prelude::XueliResult;

/// 基于 SQLite 的信号持久化存储
///
/// 对应 Python 版 `xueli/src/memory/storage/signal_store.py`
pub struct SignalStore {
    db_path: PathBuf,
    lock: Arc<Mutex<()>>,
}

/// 信号的元数据（不含 payload 本体）
#[derive(Debug, Clone)]
pub struct SignalMeta {
    pub updated_at: f64,
    pub expires_at: f64,
    pub signature: String,
}

impl SignalStore {
    pub fn new(db_path: impl AsRef<Path>) -> XueliResult<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
        }

        let store = Self {
            db_path,
            lock: Arc::new(Mutex::new(())),
        };
        store.init_db()?;
        Ok(store)
    }

    fn init_db(&self) -> XueliResult<()> {
        let conn = Connection::open(&self.db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
        conn.execute_batch("PRAGMA journal_mode=WAL")
            .map_err(|e| format!("PRAGMA 失败: {}", e))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL")
            .map_err(|e| format!("PRAGMA 失败: {}", e))?;
        conn.execute_batch("PRAGMA busy_timeout=3000")
            .map_err(|e| format!("PRAGMA 失败: {}", e))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS signals (
                signal_key TEXT PRIMARY KEY,
                signal_type TEXT NOT NULL,
                prompt_version TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                confidence REAL DEFAULT 0.0,
                created_at REAL NOT NULL,
                updated_at REAL DEFAULT 0.0,
                expires_at REAL NOT NULL
            )",
            [],
        )
        .map_err(|e| format!("建表失败: {}", e))?;

        // 兼容迁移：旧表可能缺少 updated_at 列
        let _ = conn.execute(
            "ALTER TABLE signals ADD COLUMN updated_at REAL DEFAULT 0.0",
            [],
        );

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_signals_expires_at ON signals(expires_at)",
            [],
        )
        .map_err(|e| format!("建索引失败: {}", e))?;

        Ok(())
    }

    fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }

    fn payload_signature(payload_json: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(payload_json.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    // ── 读取 ──

    /// 获取信号 payload（过期自动删除并返回 None）
    pub async fn get(&self, signal_key: &str) -> Option<serde_json::Value> {
        let key = signal_key.trim();
        if key.is_empty() {
            return None;
        }

        let db_path = self.db_path.clone();
        let key = key.to_string();

        let payload_json = tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).ok()?;
            let row = conn
                .query_row(
                    "SELECT payload_json, expires_at FROM signals WHERE signal_key=?1",
                    params![key],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
                )
                .ok()?;
            let (payload_json, expires_at) = row;
            if expires_at < Self::now() {
                let _ = conn.execute("DELETE FROM signals WHERE signal_key=?1", params![key]);
                return None;
            }
            Some(payload_json)
        })
        .await
        .unwrap_or(None);

        match payload_json {
            Some(json_str) => serde_json::from_str(&json_str).ok(),
            None => None,
        }
    }

    /// 获取信号元数据（过期自动删除并返回 None）
    pub async fn get_meta(&self, signal_key: &str) -> Option<SignalMeta> {
        let key = signal_key.trim();
        if key.is_empty() {
            return None;
        }

        let db_path = self.db_path.clone();
        let key = key.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).ok()?;
            let row = conn
                .query_row(
                    "SELECT payload_json, updated_at, expires_at FROM signals WHERE signal_key=?1",
                    params![key],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, f64>(1)?,
                            row.get::<_, f64>(2)?,
                        ))
                    },
                )
                .ok()?;
            let (payload_json, updated_at, expires_at) = row;
            if expires_at < Self::now() {
                let _ = conn.execute("DELETE FROM signals WHERE signal_key=?1", params![key]);
                return None;
            }
            Some(SignalMeta {
                updated_at,
                expires_at,
                signature: Self::payload_signature(&payload_json),
            })
        })
        .await
        .unwrap_or(None)
    }

    // ── 写入 ──

    /// 设置信号（INSERT OR REPLACE）
    pub async fn set(
        &self,
        signal_key: &str,
        signal_type: &str,
        prompt_version: &str,
        payload: &serde_json::Value,
        confidence: f64,
        ttl_seconds: f64,
    ) -> XueliResult<()> {
        let key = signal_key.trim();
        if key.is_empty() {
            return Ok(());
        }

        let key = key.to_string();
        let signal_type = signal_type.to_string();
        let prompt_version = if prompt_version.is_empty() {
            "v1".to_string()
        } else {
            prompt_version.to_string()
        };
        let payload_json =
            serde_json::to_string(payload).map_err(|e| format!("序列化失败: {}", e))?;
        let now = Self::now();
        let ttl = ttl_seconds.max(0.1);
        let expires_at = now + ttl;

        let db_path = self.db_path.clone();
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            conn.execute(
                "INSERT OR REPLACE INTO signals
                (signal_key, signal_type, prompt_version, payload_json, confidence, created_at, updated_at, expires_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    key,
                    signal_type,
                    prompt_version,
                    payload_json,
                    confidence,
                    now,
                    now,
                    expires_at,
                ],
            )
            .map_err(|e| format!("写入信号失败: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    // ── 维护 ──

    /// 清理过期信号
    pub async fn cleanup_expired(&self) -> XueliResult<()> {
        let now = Self::now();
        let db_path = self.db_path.clone();
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            conn.execute("DELETE FROM signals WHERE expires_at < ?1", params![now])
                .map_err(|e| format!("清理过期信号失败: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 删除单个信号
    pub async fn invalidate_key(&self, signal_key: &str) -> XueliResult<()> {
        let key = signal_key.trim();
        if key.is_empty() {
            return Ok(());
        }

        let db_path = self.db_path.clone();
        let key = key.to_string();
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            conn.execute("DELETE FROM signals WHERE signal_key=?1", params![key])
                .map_err(|e| format!("删除信号失败: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }

    /// 按前缀批量删除，返回删除数量
    pub async fn invalidate_prefix(&self, prefix: &str) -> XueliResult<usize> {
        let normalized = prefix.trim();
        if normalized.is_empty() {
            return Ok(0);
        }

        let db_path = self.db_path.clone();
        let like_pattern = format!("{}%", normalized);
        let _guard = self.lock.lock().await;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            let count = conn
                .execute(
                    "DELETE FROM signals WHERE signal_key LIKE ?1",
                    params![like_pattern],
                )
                .map_err(|e| format!("批量删除失败: {}", e))?;
            Ok(count)
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_store() -> (SignalStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_signals.db");
        (SignalStore::new(db_path).unwrap(), dir)
    }

    #[tokio::test]
    async fn test_set_and_get() {
        let (store, _dir) = make_store();
        let payload = serde_json::json!({"msg": "hello", "count": 42});

        store
            .set("test:key1", "test_type", "v1", &payload, 0.9, 3600.0)
            .await
            .unwrap();

        let result = store.get("test:key1").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap()["msg"], "hello");
    }

    #[tokio::test]
    async fn test_expired_entry() {
        let (store, _dir) = make_store();
        let payload = serde_json::json!({"x": 1});

        store
            .set("test:exp", "t", "v1", &payload, 0.5, 0.2)
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let result = store.get("test:exp").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_meta() {
        let (store, _dir) = make_store();
        let payload = serde_json::json!({"data": "test"});

        store
            .set("test:meta", "meta_type", "v2", &payload, 0.8, 3600.0)
            .await
            .unwrap();

        let meta = store.get_meta("test:meta").await;
        assert!(meta.is_some());
        assert!(!meta.unwrap().signature.is_empty());
    }

    #[tokio::test]
    async fn test_invalidate_key() {
        let (store, _dir) = make_store();
        let payload = serde_json::json!({});

        store
            .set("test:del", "t", "v1", &payload, 0.5, 3600.0)
            .await
            .unwrap();
        assert!(store.get("test:del").await.is_some());

        store.invalidate_key("test:del").await.unwrap();
        assert!(store.get("test:del").await.is_none());
    }

    #[tokio::test]
    async fn test_invalidate_prefix() {
        let (store, _dir) = make_store();
        let payload = serde_json::json!({});

        for i in 0..3 {
            store
                .set(&format!("pfx:key{}", i), "t", "v1", &payload, 0.5, 3600.0)
                .await
                .unwrap();
        }
        store
            .set("other:key", "t", "v1", &payload, 0.5, 3600.0)
            .await
            .unwrap();

        let deleted = store.invalidate_prefix("pfx:").await.unwrap();
        assert_eq!(deleted, 3);
        assert!(store.get("pfx:key0").await.is_none());
        assert!(store.get("other:key").await.is_some());
    }

    #[tokio::test]
    async fn test_empty_key_ignored() {
        let (store, _dir) = make_store();
        let payload = serde_json::json!({});

        store.set("", "t", "v1", &payload, 0.5, 1.0).await.unwrap();
        assert!(store.get("").await.is_none());
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let (store, _dir) = make_store();
        let payload = serde_json::json!({"x": 1});

        store
            .set("test:cleanup_exp", "t", "v1", &payload, 0.5, 0.1)
            .await
            .unwrap();
        store
            .set("test:cleanup_valid", "t", "v1", &payload, 0.5, 3600.0)
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        store.cleanup_expired().await.unwrap();
        assert!(store.get("test:cleanup_exp").await.is_none());
        assert!(store.get("test:cleanup_valid").await.is_some());
    }
}
