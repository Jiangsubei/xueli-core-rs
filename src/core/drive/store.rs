//! DriveStore — 内驱力状态持久化，原子写入，支持从旧 MoodStore 格式迁移。

use std::path::PathBuf;

use chrono::Utc;
use tokio::fs;
use tracing::info;

use super::models::{
    AffectiveState, DriveSnapshot, MotivationalDimension, MotivationalKey, PADVector,
};

/// 按作用域键在 `_drive_states/` 目录中持久化 DriveSnapshot，原子写入。
///
/// 同时提供从旧 MoodStore 格式一次性迁移的能力。
pub struct DriveStore {
    base_path: PathBuf,
}

impl DriveStore {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        let base = base_path.into();
        Self {
            base_path: base.join("_drive_states"),
        }
    }

    fn sanitize_key(key: &str) -> String {
        let safe: String = key.replace('/', "_").replace('\\', "_").replace(':', "_");
        if safe.is_empty() {
            "default".to_string()
        } else {
            safe
        }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.base_path
            .join(format!("{}.json", Self::sanitize_key(key)))
    }

    async fn ensure_dir(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.base_path).await
    }

    /// 从磁盘加载指定 key 的内驱力状态快照。
    pub async fn load(&self, key: &str) -> Option<DriveSnapshot> {
        let _ = self.ensure_dir().await;
        let path = self.path_for(key);
        if !path.exists() {
            return None;
        }
        let data = match tokio::task::spawn_blocking(move || {
            std::fs::read_to_string(&path).map(|s| serde_json::from_str::<serde_json::Value>(&s))
        })
        .await
        {
            Ok(Ok(Ok(v))) => v,
            _ => return None,
        };

        let obj = match data.as_object() {
            Some(o) => o,
            None => return None,
        };

        // 版本检测：如果包含旧 MoodStore 格式字段，尝试迁移
        if obj.contains_key("valence") && !obj.contains_key("pad") {
            return Some(Self::migrate_from_mood_store(obj, key));
        }

        match serde_json::from_value::<DriveSnapshot>(data) {
            Ok(snap) => Some(snap),
            Err(_) => None,
        }
    }

    /// 原子写入内驱力状态快照到磁盘。
    pub async fn save(&self, key: &str, snapshot: &DriveSnapshot) -> std::io::Result<()> {
        self.ensure_dir().await?;
        let path = self.path_for(key);
        let tmp_path = PathBuf::from(format!("{}.tmp", path.display()));
        let data = serde_json::to_string_pretty(snapshot).unwrap_or_default();

        // 在 spawn_blocking 中执行原子写入
        let tmp_path_clone = tmp_path.clone();
        let path_clone = path.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::write(&tmp_path_clone, data.as_bytes())?;
            std::fs::rename(&tmp_path_clone, &path_clone)
        })
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))??;

        Ok(())
    }

    /// 从旧 MoodStore JSON 格式迁移到 DriveSnapshot。
    ///
    /// 旧格式: {valence, arousal, energy, updated_at}
    fn migrate_from_mood_store(
        obj: &serde_json::Map<String, serde_json::Value>,
        scope_key: &str,
    ) -> DriveSnapshot {
        let valence = obj.get("valence").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let arousal = obj.get("arousal").and_then(|v| v.as_f64()).unwrap_or(0.5);
        let energy = obj.get("energy").and_then(|v| v.as_f64()).unwrap_or(0.8);
        let updated_at = obj
            .get("updated_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let updated_at = if updated_at.is_empty() {
            Utc::now().to_rfc3339()
        } else {
            updated_at
        };

        info!("[DriveStore] 从 MoodStore 格式迁移: key={}", scope_key);

        let mut motivational = std::collections::HashMap::new();
        motivational.insert(
            MotivationalKey::SocialDrive.as_str().to_string(),
            MotivationalDimension {
                baseline: 0.5,
                ..Default::default()
            },
        );
        motivational.insert(
            MotivationalKey::Curiosity.as_str().to_string(),
            MotivationalDimension {
                baseline: 0.5,
                ..Default::default()
            },
        );
        motivational.insert(
            MotivationalKey::Caution.as_str().to_string(),
            MotivationalDimension {
                baseline: 0.3,
                ..Default::default()
            },
        );
        motivational.insert(
            MotivationalKey::Proactivity.as_str().to_string(),
            MotivationalDimension {
                baseline: energy,
                ..Default::default()
            },
        );

        DriveSnapshot {
            affective: AffectiveState {
                pad: PADVector {
                    valence,
                    arousal,
                    dominance: 0.5,
                },
                updated_at: updated_at.clone(),
            },
            motivational,
            relational: std::collections::HashMap::new(),
            event_rules: super::event_rules::build_default_rule_set(),
            scope_key: scope_key.to_string(),
            version: 1,
            created_at: updated_at.clone(),
            updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_key() {
        assert_eq!(DriveStore::sanitize_key("a/b:c"), "a_b_c");
        assert_eq!(DriveStore::sanitize_key(""), "default");
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = DriveStore::new(dir.path().to_path_buf());
        let snapshot = DriveSnapshot::create_default("test_scope");
        store.save("test_scope", &snapshot).await.unwrap();
        let loaded = store.load("test_scope").await;
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.scope_key, "test_scope");
    }

    #[tokio::test]
    async fn test_load_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let store = DriveStore::new(dir.path().to_path_buf());
        let loaded = store.load("nonexistent").await;
        assert!(loaded.is_none());
    }
}
