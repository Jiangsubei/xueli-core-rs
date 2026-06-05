use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// 角色卡 — 定义 bot 的角色身份及其对特定用户的适应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterCard {
    pub name: String,
    pub age: Option<u32>,
    pub gender: Option<String>,
    pub personality: Vec<String>,
    pub interests: Vec<String>,
    pub speaking_style: String,
    pub background_story: Option<String>,
    pub relationships: Vec<CharacterRelationship>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterRelationship {
    pub person_name: String,
    pub relation_type: String,
    pub description: String,
}

/// 用户的角色快照（存储层）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterCardSnapshot {
    pub user_id: String,
    pub core_traits: Vec<String>,
    pub tone_preferences: Vec<String>,
    pub bot_persona_hints: Vec<String>,
    pub explicit_feedback_count: usize,
    pub intimacy_level: f64,
    pub relationship_stage: String,
    pub emotional_trend: String,
    pub updated_at: String,
}

impl Default for CharacterCardSnapshot {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            core_traits: Vec::new(),
            tone_preferences: Vec::new(),
            bot_persona_hints: Vec::new(),
            explicit_feedback_count: 0,
            intimacy_level: 0.0,
            relationship_stage: "stranger".into(),
            emotional_trend: String::new(),
            updated_at: String::new(),
        }
    }
}

/// 角色卡服务 — 管理角色定义、用户快照和关系亲密度。
pub struct CharacterCardService {
    /// 角色基础定义
    card: CharacterCard,
    /// 用户快照缓存
    snapshots: Mutex<HashMap<String, CharacterCardSnapshot>>,
    /// 持久化目录
    storage_dir: String,
}

impl CharacterCardService {
    pub fn new(card: CharacterCard, storage_dir: &str) -> Self {
        let mut svc = Self {
            card,
            snapshots: Mutex::new(HashMap::new()),
            storage_dir: storage_dir.to_string(),
        };
        svc.load_all();
        svc
    }

    pub fn default_card() -> CharacterCard {
        CharacterCard {
            name: String::new(),
            age: None,
            gender: None,
            personality: vec!["友好".to_string(), "耐心".to_string(), "幽默".to_string()],
            interests: vec![
                "聊天".to_string(),
                "学习".to_string(),
                "帮助他人".to_string(),
            ],
            speaking_style: "自然、随和、略带俏皮".to_string(),
            background_story: None,
            relationships: Vec::new(),
        }
    }

    /// 获取角色基础定义
    pub fn get_card(&self) -> &CharacterCard {
        &self.card
    }

    /// 获取用户快照（不存在则创建默认）
    pub fn get_snapshot(&self, user_id: &str) -> CharacterCardSnapshot {
        let mut snapshots = self.snapshots.lock().unwrap();
        snapshots
            .entry(user_id.to_string())
            .or_insert_with(|| CharacterCardSnapshot {
                user_id: user_id.to_string(),
                ..Default::default()
            })
            .clone()
    }

    /// 更新用户快照
    pub fn update_snapshot(&self, snapshot: CharacterCardSnapshot) {
        let mut snapshots = self.snapshots.lock().unwrap();
        snapshots.insert(snapshot.user_id.clone(), snapshot.clone());
        self.save_one(&snapshot);
    }

    /// 记录反馈并调整亲密度
    pub fn record_feedback(
        &self,
        user_id: &str,
        sentiment: f64,
        traits: &[&str],
        preferences: &[&str],
    ) {
        let mut snapshot = self.get_snapshot(user_id);
        snapshot.explicit_feedback_count += 1;

        // 调整亲密度
        let delta = sentiment * 0.02;
        snapshot.intimacy_level = (snapshot.intimacy_level + delta).clamp(0.0, 1.0);

        // 更新关系阶段
        snapshot.relationship_stage = Self::resolve_stage(snapshot.intimacy_level);

        // 更新偏好
        for pref in preferences {
            let p = pref.to_string();
            if !snapshot.tone_preferences.contains(&p) {
                snapshot.tone_preferences.push(p);
            }
        }
        if !traits.is_empty() {
            snapshot.core_traits = traits.iter().map(|s| s.to_string()).collect();
        }

        snapshot.updated_at = chrono::Utc::now().to_rfc3339();
        self.update_snapshot(snapshot);
    }

    /// 根据亲密度解析关系阶段
    fn resolve_stage(intimacy: f64) -> String {
        if intimacy >= 0.9 {
            "intimate"
        } else if intimacy >= 0.8 {
            "close_friend"
        } else if intimacy >= 0.5 {
            "friend"
        } else if intimacy >= 0.2 {
            "acquaintance"
        } else if intimacy >= 0.1 {
            "met_before"
        } else {
            "stranger"
        }
        .into()
    }

    fn snapshot_path(&self, user_id: &str) -> String {
        format!("{}/{}.json", self.storage_dir, user_id)
    }

    fn save_one(&self, snapshot: &CharacterCardSnapshot) {
        let path = self.snapshot_path(&snapshot.user_id);
        let tmp = format!("{}.tmp", path);
        if let Ok(data) = serde_json::to_string_pretty(snapshot) {
            let _ = std::fs::write(&tmp, data);
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn load_all(&mut self) {
        if let Ok(entries) = std::fs::read_dir(&self.storage_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "json") {
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if let Ok(snap) = serde_json::from_str::<CharacterCardSnapshot>(&data) {
                            self.snapshots
                                .lock()
                                .unwrap()
                                .insert(snap.user_id.clone(), snap);
                        }
                    }
                }
            }
        }
    }
}

impl Default for CharacterCardService {
    fn default() -> Self {
        Self::new(Self::default_card(), "data/character_cards")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_card() {
        let card = CharacterCardService::default_card();
        assert!(card.name.is_empty());
        assert!(!card.personality.is_empty());
    }

    #[test]
    fn test_resolve_stage() {
        assert_eq!(CharacterCardService::resolve_stage(0.0), "stranger");
        assert_eq!(CharacterCardService::resolve_stage(0.3), "acquaintance");
        assert_eq!(CharacterCardService::resolve_stage(0.6), "friend");
        assert_eq!(CharacterCardService::resolve_stage(0.85), "close_friend");
        assert_eq!(CharacterCardService::resolve_stage(0.95), "intimate");
    }

    #[test]
    fn test_get_and_update_snapshot() {
        let dir = tempfile::TempDir::new().unwrap();
        let card = CharacterCardService::default_card();
        let svc = CharacterCardService::new(card, dir.path().to_str().unwrap());

        let snap = svc.get_snapshot("u1");
        assert_eq!(snap.relationship_stage, "stranger");

        svc.record_feedback("u1", 0.8, &["幽默"], &["轻松"]);
        let updated = svc.get_snapshot("u1");
        assert!(updated.intimacy_level > 0.0);
        assert_eq!(updated.explicit_feedback_count, 1);
    }
}
