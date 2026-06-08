use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::core::types::MoodState;
use crate::prelude::XueliResult;

/// 心情状态持久化存储（SQLite）
///
/// 对应 Python 版 `xueli/src/core/mood_store.py`
/// Python 原版使用 JSON 文件，本项目改为 SQLite 存储。
pub struct MoodStore {
    db_path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl MoodStore {
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
        conn.execute_batch("PRAGMA busy_timeout=3000")
            .map_err(|e| format!("PRAGMA 失败: {}", e))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS mood_states (
                scope_key TEXT PRIMARY KEY,
                valence REAL NOT NULL DEFAULT 0.0,
                arousal REAL NOT NULL DEFAULT 0.5,
                energy REAL NOT NULL DEFAULT 1.0,
                updated_at TEXT NOT NULL DEFAULT ''
            )",
            [],
        )
        .map_err(|e| format!("建表失败: {}", e))?;

        Ok(())
    }

    // ── 读取 ──

    /// 加载指定 key 的心情状态
    pub fn load(&self, scope_key: &str) -> Option<MoodState> {
        let key = Self::sanitize_key(scope_key);
        let conn = Connection::open(&self.db_path).ok()?;
        conn.query_row(
            "SELECT valence, arousal, energy, updated_at FROM mood_states WHERE scope_key=?1",
            params![key],
            |row| {
                Ok(MoodState {
                    valence: row.get(0).unwrap_or(0.0),
                    arousal: row.get(1).unwrap_or(0.5),
                    energy: row.get(2).unwrap_or(1.0),
                    updated_at: row.get(3).unwrap_or_default(),
                    ..Default::default()
                })
            },
        )
        .ok()
    }

    /// 异步加载
    pub async fn load_async(&self, scope_key: &str) -> Option<MoodState> {
        let db_path = self.db_path.clone();
        let key = Self::sanitize_key(scope_key);

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).ok()?;
            conn.query_row(
                "SELECT valence, arousal, energy, updated_at FROM mood_states WHERE scope_key=?1",
                params![key],
                |row| {
                    Ok(MoodState {
                        valence: row.get(0).unwrap_or(0.0),
                        arousal: row.get(1).unwrap_or(0.5),
                        energy: row.get(2).unwrap_or(1.0),
                        updated_at: row.get(3).unwrap_or_default(),
                        ..Default::default()
                    })
                },
            )
            .ok()
        })
        .await
        .unwrap_or(None)
    }

    // ── 写入 ──

    /// 保存心情状态（同步，原子写入）
    pub fn save(&self, scope_key: &str, state: &MoodState) -> XueliResult<()> {
        let key = Self::sanitize_key(scope_key);
        let now = Utc::now().to_rfc3339();
        let updated_at = if state.updated_at.is_empty() {
            &now
        } else {
            &state.updated_at
        };

        let conn = Connection::open(&self.db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
        conn.execute(
            "INSERT OR REPLACE INTO mood_states (scope_key, valence, arousal, energy, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key, state.valence, state.arousal, state.energy, updated_at],
        )
        .map_err(|e| format!("保存心情状态失败: {}", e))?;

        Ok(())
    }

    /// 异步保存
    pub async fn save_async(&self, scope_key: &str, state: &MoodState) -> XueliResult<()> {
        let key = Self::sanitize_key(scope_key);
        let now = Utc::now().to_rfc3339();
        let updated_at = if state.updated_at.is_empty() {
            &now
        } else {
            &state.updated_at
        };

        let db_path = self.db_path.clone();
        let key = key.to_string();
        let valence = state.valence;
        let arousal = state.arousal;
        let energy = state.energy;
        let updated_at = updated_at.to_string();

        let _guard = self.lock.lock().await;

        let result: XueliResult<()> = tokio::task::spawn_blocking(move || {
            let conn =
                Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            conn.execute(
                "INSERT OR REPLACE INTO mood_states (scope_key, valence, arousal, energy, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![key, valence, arousal, energy, updated_at],
            )
            .map_err(|e| format!("保存心情状态失败: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?;

        result?;
        Ok(())
    }

    // ── 辅助 ──

    /// 清理 key 中的不安全字符
    fn sanitize_key(key: &str) -> String {
        let safe = key.replace('/', "_").replace('\\', "_").replace(':', "_");
        if safe.is_empty() {
            "default".to_string()
        } else {
            safe
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_store() -> (MoodStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_moods.db");
        (MoodStore::new(db_path).unwrap(), dir)
    }

    #[test]
    fn test_save_and_load() {
        let (store, _dir) = make_store();
        let state = MoodState {
            valence: 0.5,
            arousal: 0.3,
            energy: 0.8,
            updated_at: "2024-01-01T00:00:00+00:00".to_string(),
            ..Default::default()
        };

        store.save("user:12345", &state).unwrap();
        let loaded = store.load("user:12345").unwrap();

        assert!((loaded.valence - 0.5).abs() < 1e-9);
        assert!((loaded.arousal - 0.3).abs() < 1e-9);
        assert!((loaded.energy - 0.8).abs() < 1e-9);
    }

    #[test]
    fn test_overwrite() {
        let (store, _dir) = make_store();
        let state1 = MoodState {
            valence: 0.2,
            arousal: 0.5,
            energy: 0.9,
            updated_at: String::new(),
            ..Default::default()
        };
        store.save("scope:g1", &state1).unwrap();

        let state2 = MoodState {
            valence: -0.3,
            arousal: 0.7,
            energy: 0.4,
            updated_at: "2024-06-01T00:00:00+00:00".to_string(),
            ..Default::default()
        };
        store.save("scope:g1", &state2).unwrap();

        let loaded = store.load("scope:g1").unwrap();
        assert!((loaded.valence - (-0.3)).abs() < 1e-9);
        assert!((loaded.energy - 0.4).abs() < 1e-9);
    }

    #[test]
    fn test_load_nonexistent() {
        let (store, _dir) = make_store();
        assert!(store.load("nonexistent").is_none());
    }

    #[test]
    fn test_sanitize_key() {
        let key = "group:abc/def\\ghi";
        let safe = MoodStore::sanitize_key(key);
        assert!(!safe.contains('/'));
        assert!(!safe.contains('\\'));
        assert!(!safe.contains(':'));
    }

    #[tokio::test]
    async fn test_save_async_and_load_async() {
        let (store, _dir) = make_store();
        let state = MoodState {
            valence: 0.6,
            arousal: 0.4,
            energy: 0.7,
            updated_at: String::new(),
            ..Default::default()
        };

        store.save_async("async:key", &state).await.unwrap();
        let loaded = store.load_async("async:key").await.unwrap();
        assert!((loaded.valence - 0.6).abs() < 1e-9);
    }
}
