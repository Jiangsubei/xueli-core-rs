use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::prelude::XueliResult;

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
    #[serde(default)]
    pub stable_signal_count: usize,
    #[serde(default)]
    pub behavior_habits: Vec<String>,
    #[serde(default)]
    pub prediction_feedback_summary: String,
    #[serde(default)]
    pub relationship_tone_hint: String,
    #[serde(default)]
    pub relationship_state_summary: String,
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
            stable_signal_count: 0,
            behavior_habits: Vec::new(),
            prediction_feedback_summary: String::new(),
            relationship_tone_hint: String::new(),
            relationship_state_summary: String::new(),
        }
    }
}

// ── 内部持久化类型 ──

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FeedbackEntry {
    text: String,
    category: String,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SignalEntry {
    signal: String,
    weight: i32,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmotionEntry {
    tone: String,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RelationshipProfileInternal {
    intimacy_level: f64,
    total_interactions: u64,
    reply_positive_count: u64,
    reply_negative_count: u64,
    reply_repair_count: u64,
    friction_signals: u64,
    last_feedback_label: String,
    last_reply_intent: String,
    relationship_stage: String,
    last_interaction_at: String,
    last_intimacy_change: String,
}

impl RelationshipProfileInternal {
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

    fn update_stage(&mut self) {
        self.relationship_stage = Self::resolve_stage(self.intimacy_level);
    }
}

/// 每用户持久化载荷
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct UserPayload {
    #[serde(default)]
    explicit_feedback: Vec<FeedbackEntry>,
    #[serde(default)]
    stable_signals: Vec<SignalEntry>,
    #[serde(default)]
    emotional_history: Vec<EmotionEntry>,
    #[serde(default)]
    relationship_profile: Option<RelationshipProfileInternal>,
    #[serde(default)]
    snapshot: Option<CharacterCardSnapshot>,
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

        let delta = sentiment * 0.02;
        snapshot.intimacy_level = (snapshot.intimacy_level + delta).clamp(0.0, 1.0);
        snapshot.relationship_stage = Self::resolve_stage(snapshot.intimacy_level);

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
    pub fn resolve_stage(intimacy: f64) -> String {
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

    // ── 新增方法 ──────────────────────────────────────────

    /// 记录用户显式反馈类别（category-based feedback counting）
    pub fn record_explicit_feedback_category(
        &self,
        user_id: &str,
        category: &str,
    ) -> XueliResult<()> {
        let normalized = category.trim();
        if normalized.is_empty() {
            return Ok(());
        }
        let mut payload = self.load_payload(user_id);
        payload.explicit_feedback.push(FeedbackEntry {
            text: format!("[llm]{}", normalized),
            category: normalized.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        });
        self.save_payload(user_id, &payload);
        Ok(())
    }

    /// 记录交互信号（per-signal interaction tracking with weights）
    pub fn record_interaction_signal(&self, user_id: &str, signal_name: &str) -> XueliResult<()> {
        let normalized = signal_name.trim();
        if normalized.is_empty() {
            return Ok(());
        }
        let mut payload = self.load_payload(user_id);
        payload.stable_signals.push(SignalEntry {
            signal: normalized.to_string(),
            weight: 1,
            created_at: chrono::Utc::now().to_rfc3339(),
        });
        self.save_payload(user_id, &payload);
        Ok(())
    }

    /// 记录回复反馈（feedback label tracking + intimacy delta）
    pub fn record_reply_feedback(
        &self,
        user_id: &str,
        feedback_label: &str,
        score: f64,
    ) -> XueliResult<()> {
        let label = feedback_label.trim().to_lowercase();
        let mut payload = self.load_payload(user_id);

        let mut profile = payload.relationship_profile.take().unwrap_or_default();

        if label == "positive" {
            profile.reply_positive_count += 1;
        } else if label == "negative" {
            profile.reply_negative_count += 1;
        } else if label == "repair" {
            profile.reply_repair_count += 1;
        }
        profile.last_feedback_label = label.clone();

        let is_friction = label == "negative" || label == "repair";
        let delta = if !is_friction && score > 0.0 {
            score * 0.03
        } else if is_friction {
            -score.abs() * 0.05
        } else {
            0.0
        };

        if delta != 0.0 {
            profile.intimacy_level = (profile.intimacy_level + delta).clamp(0.0, 1.0);
            profile.total_interactions += 1;
            let now = chrono::Utc::now().to_rfc3339();
            profile.last_interaction_at = now.clone();
            profile.last_intimacy_change = now;
            if is_friction {
                profile.friction_signals += 1;
            } else {
                profile.friction_signals = profile.friction_signals.saturating_sub(1);
            }
            profile.update_stage();
        }

        payload.relationship_profile = Some(profile);
        self.save_payload(user_id, &payload);
        self.refresh_snapshot(user_id);

        Ok(())
    }

    /// 记录用户情绪（emotional history sliding window, last 10 entries）
    pub fn record_emotion(&self, user_id: &str, emotion_label: &str) -> XueliResult<()> {
        let tone = emotion_label.trim();
        if tone.is_empty() {
            return Ok(());
        }
        let mut payload = self.load_payload(user_id);
        payload.emotional_history.push(EmotionEntry {
            tone: tone.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        });
        if payload.emotional_history.len() > 10 {
            payload.emotional_history = payload
                .emotional_history
                .split_off(payload.emotional_history.len() - 10);
        }
        self.save_payload(user_id, &payload);
        Ok(())
    }

    /// 注入用户画像信号到快照
    pub fn update_snapshot_signals(
        &self,
        user_id: &str,
        user_profile_signal: Option<&HashMap<String, String>>,
    ) -> XueliResult<CharacterCardSnapshot> {
        let mut payload = self.load_payload(user_id);
        let mut snapshot = self.get_snapshot(user_id);

        if let Some(signal) = user_profile_signal {
            let read = signal.get("relationship_read").cloned().unwrap_or_default();
            let safety = signal.get("emotional_safety").cloned().unwrap_or_default();
            let guidance = signal.get("tone_guidance").cloned().unwrap_or_default();

            let mut parts = vec![read];
            if !safety.is_empty() {
                parts.push(format!("情绪安全感={}", safety));
            }
            if !guidance.is_empty() {
                parts.push(guidance);
            }
            snapshot.relationship_state_summary = parts
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("；");
        }

        snapshot.updated_at = chrono::Utc::now().to_rfc3339();
        payload.snapshot = Some(snapshot.clone());
        self.save_payload(user_id, &payload);
        self.update_snapshot(snapshot.clone());

        Ok(snapshot)
    }

    /// 分析近期情绪历史趋势方向
    pub fn get_emotional_trend(&self, user_id: &str) -> String {
        let payload = self.load_payload(user_id);
        if payload.emotional_history.len() < 2 {
            return String::new();
        }

        let recent: Vec<&str> = payload
            .emotional_history
            .iter()
            .rev()
            .take(5)
            .map(|e| e.tone.as_str())
            .collect();

        let positive_tones = ["happy", "开心", "喜欢", "温暖", "感动", "兴奋", "期待"];
        let negative_tones = [
            "sad", "难过", "伤心", "生气", "愤怒", "焦虑", "害怕", "疲惫",
        ];

        let pos_count = recent
            .iter()
            .filter(|t| positive_tones.iter().any(|p| t.contains(p)))
            .count();
        let neg_count = recent
            .iter()
            .filter(|t| negative_tones.iter().any(|n| t.contains(n)))
            .count();

        if pos_count > neg_count && pos_count > recent.len() / 2 {
            "趋向积极".to_string()
        } else if neg_count > pos_count && neg_count > recent.len() / 2 {
            "趋向消极".to_string()
        } else {
            "情绪平稳".to_string()
        }
    }

    /// 刷新用户快照：计算预测命中率、信号计数器，构建完整快照
    pub fn refresh_snapshot(&self, user_id: &str) -> CharacterCardSnapshot {
        let mut payload = self.load_payload(user_id);

        // 统计显式反馈类别
        let mut category_counts: HashMap<String, usize> = HashMap::new();
        for entry in &payload.explicit_feedback {
            *category_counts.entry(entry.category.clone()).or_insert(0) += 1;
        }

        // 统计信号
        let mut signal_counts: HashMap<String, i32> = HashMap::new();
        for entry in &payload.stable_signals {
            *signal_counts.entry(entry.signal.clone()).or_insert(0) += entry.weight;
        }

        // 计算预测命中率
        let prediction_met = signal_counts.get("prediction:met").copied().unwrap_or(0) as usize;
        let prediction_failed =
            signal_counts.get("prediction:failed").copied().unwrap_or(0) as usize;
        let prediction_total = prediction_met + prediction_failed;
        let prediction_feedback_summary = if prediction_total > 0 {
            let rate = prediction_met as f64 / prediction_total as f64;
            format!(
                "历史预测命中率={:.0}%（命中{}次/失败{}次）",
                rate * 100.0,
                prediction_met,
                prediction_failed
            )
        } else {
            String::new()
        };

        let profile = payload
            .relationship_profile
            .as_ref()
            .cloned()
            .unwrap_or_default();

        let snapshot = CharacterCardSnapshot {
            user_id: user_id.to_string(),
            core_traits: Vec::new(),
            tone_preferences: Vec::new(),
            bot_persona_hints: Vec::new(),
            explicit_feedback_count: category_counts.values().sum(),
            stable_signal_count: signal_counts.values().sum::<i32>() as usize,
            intimacy_level: profile.intimacy_level,
            relationship_stage: if profile.relationship_stage.is_empty() {
                Self::resolve_stage(profile.intimacy_level)
            } else {
                profile.relationship_stage
            },
            emotional_trend: self.get_emotional_trend(user_id),
            updated_at: chrono::Utc::now().to_rfc3339(),
            prediction_feedback_summary,
            relationship_tone_hint: String::new(),
            behavior_habits: Vec::new(),
            relationship_state_summary: String::new(),
        };

        payload.snapshot = Some(snapshot.clone());
        self.save_payload(user_id, &payload);
        self.update_snapshot(snapshot.clone());

        snapshot
    }

    /// 综合反馈应用：在一次调用中处理回复效果的所有维度
    ///
    /// 参数：
    /// - `score`: 回复效度评分，正值 = 正向，负值 = 负向/需修复
    /// - `feedback_label`: 反馈标签 (positive/negative/repair)
    /// - `intent_label`: 回复意图标签
    /// - `style_label`: 风格标签
    /// - `emotion_label`: 用户情绪标签
    /// - `expected_effect`: 预期效果
    /// - `prediction_met`: 预测是否命中
    pub fn apply_reply_effect_bundle(
        &self,
        user_id: &str,
        score: f64,
        feedback_label: &str,
        intent_label: &str,
        style_label: &str,
        emotion_label: &str,
        expected_effect: &str,
        prediction_met: Option<bool>,
    ) -> XueliResult<()> {
        let mut payload = self.load_payload(user_id);
        let now = chrono::Utc::now().to_rfc3339();

        let normalized_feedback = feedback_label.trim().to_lowercase();
        let normalized_intent = intent_label.trim();
        let score_label = if score > 0.3 {
            "positive"
        } else if score < -0.3 {
            "negative"
        } else if score < 0.0 {
            "repair"
        } else {
            "neutral"
        };

        // 记录评分信号
        if score_label == "negative" {
            payload.stable_signals.push(SignalEntry {
                signal: "reply_negative".to_string(),
                weight: 1,
                created_at: now.clone(),
            });
        } else if score_label == "positive" {
            payload.stable_signals.push(SignalEntry {
                signal: "reply_positive".to_string(),
                weight: 1,
                created_at: now.clone(),
            });
        } else if score_label == "repair" {
            payload.stable_signals.push(SignalEntry {
                signal: "reply_repair".to_string(),
                weight: 1,
                created_at: now.clone(),
            });
        }

        // 记录意图+反馈组合信号
        if !normalized_intent.is_empty() && !normalized_feedback.is_empty() {
            payload.stable_signals.push(SignalEntry {
                signal: format!("reply_effect:{}:{}", normalized_intent, normalized_feedback),
                weight: 1,
                created_at: now.clone(),
            });
        }

        // 记录预期效果
        let normalized_expected = expected_effect.trim().to_lowercase();
        if !normalized_expected.is_empty() {
            let actual = if score > 0.0 { "satisfy" } else { "clarify" };
            payload.stable_signals.push(SignalEntry {
                signal: format!("expected_actual:{}:{}", normalized_expected, actual),
                weight: 1,
                created_at: now.clone(),
            });
        }

        // 记录预测结果
        if let Some(met) = prediction_met {
            let suffix = if met { "met" } else { "failed" };
            payload.stable_signals.push(SignalEntry {
                signal: format!("prediction:{}", suffix),
                weight: 1,
                created_at: now.clone(),
            });
        }

        // 更新关系亲密度
        if !normalized_feedback.is_empty() {
            let mut profile = payload.relationship_profile.take().unwrap_or_default();

            match normalized_feedback.as_str() {
                "positive" => profile.reply_positive_count += 1,
                "negative" => profile.reply_negative_count += 1,
                "repair" => profile.reply_repair_count += 1,
                _ => {}
            }
            profile.last_feedback_label = normalized_feedback;
            if !normalized_intent.is_empty() {
                profile.last_reply_intent = normalized_intent.to_string();
            }

            let is_friction = score_label == "negative" || score_label == "repair";
            let delta = if score_label == "positive" {
                score.abs() * 0.03
            } else if score_label == "repair" {
                -score.abs() * 0.025
            } else if score_label == "negative" {
                -score.abs() * 0.05
            } else {
                0.0
            };

            if delta != 0.0 {
                profile.intimacy_level = (profile.intimacy_level + delta).clamp(0.0, 1.0);
                profile.total_interactions += 1;
                profile.last_interaction_at = now.clone();
                profile.last_intimacy_change = now.clone();
                if is_friction {
                    profile.friction_signals += 1;
                } else {
                    profile.friction_signals = profile.friction_signals.saturating_sub(1);
                }
                profile.update_stage();
            }

            payload.relationship_profile = Some(profile);
        }

        // 记录风格反馈
        if !style_label.trim().is_empty() {
            payload.explicit_feedback.push(FeedbackEntry {
                text: "[llm feedback triage]".to_string(),
                category: style_label.trim().to_string(),
                created_at: now.clone(),
            });
        }

        // 记录情绪历史
        if !emotion_label.trim().is_empty() {
            payload.emotional_history.push(EmotionEntry {
                tone: emotion_label.trim().to_string(),
                created_at: now,
            });
            if payload.emotional_history.len() > 10 {
                payload.emotional_history = payload
                    .emotional_history
                    .split_off(payload.emotional_history.len() - 10);
            }
        }

        self.save_payload(user_id, &payload);
        self.refresh_snapshot(user_id);

        Ok(())
    }

    // ── 载荷管理 ──

    fn payload_path(&self, user_id: &str) -> String {
        format!("{}/{}_payload.json", self.storage_dir, user_id)
    }

    fn load_payload(&self, user_id: &str) -> UserPayload {
        let path = self.payload_path(user_id);
        match std::fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => UserPayload::default(),
        }
    }

    fn save_payload(&self, user_id: &str, payload: &UserPayload) {
        let path = self.payload_path(user_id);
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content = serde_json::to_string_pretty(payload).unwrap_or_default();
        let tmp = format!("{}.tmp", path);
        let _ = std::fs::write(&tmp, &content);
        let _ = std::fs::rename(&tmp, &path);
    }

    // ── 快照文件读写 ──

    fn snapshot_path(&self, user_id: &str) -> String {
        format!("{}/{}.json", self.storage_dir, user_id)
    }

    fn save_one(&self, snapshot: &CharacterCardSnapshot) {
        let path = self.snapshot_path(&snapshot.user_id);
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
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
    use tempfile::TempDir;

    fn make_svc() -> (CharacterCardService, TempDir) {
        let dir = TempDir::new().unwrap();
        let card = CharacterCardService::default_card();
        let svc = CharacterCardService::new(card, dir.path().to_str().unwrap());
        (svc, dir)
    }

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
        let (svc, _dir) = make_svc();

        let snap = svc.get_snapshot("u1");
        assert_eq!(snap.relationship_stage, "stranger");

        svc.record_feedback("u1", 0.8, &["幽默"], &["轻松"]);
        let updated = svc.get_snapshot("u1");
        assert!(updated.intimacy_level > 0.0);
        assert_eq!(updated.explicit_feedback_count, 1);
    }

    #[test]
    fn test_record_explicit_feedback_category() {
        let (svc, _dir) = make_svc();
        svc.record_explicit_feedback_category("u1", "喜欢轻松语气")
            .unwrap();
        svc.record_explicit_feedback_category("u1", "讨厌长篇大论")
            .unwrap();

        let snapshot = svc.refresh_snapshot("u1");
        assert_eq!(snapshot.explicit_feedback_count, 2);
    }

    #[test]
    fn test_record_interaction_signal() {
        let (svc, _dir) = make_svc();
        svc.record_interaction_signal("u2", "frequent_reply")
            .unwrap();
        svc.record_interaction_signal("u2", "quick_response")
            .unwrap();

        let snapshot = svc.refresh_snapshot("u2");
        assert_eq!(snapshot.stable_signal_count, 2);
    }

    #[test]
    fn test_record_reply_feedback() {
        let (svc, _dir) = make_svc();

        svc.record_reply_feedback("u3", "positive", 0.8).unwrap();
        let snapshot = svc.get_snapshot("u3");
        assert!(snapshot.intimacy_level > 0.0);
        assert_eq!(snapshot.stable_signal_count, 0); // 只更新关系，不写入 stable_signals
    }

    #[test]
    fn test_record_emotion_sliding_window() {
        let (svc, _dir) = make_svc();

        for i in 0..12 {
            svc.record_emotion("u4", &format!("emotion_{}", i)).unwrap();
        }

        let trend = svc.get_emotional_trend("u4");
        // 滑动窗口保留最近 10 条
        assert!(!trend.is_empty());
    }

    #[test]
    fn test_get_emotional_trend() {
        let (svc, _dir) = make_svc();

        svc.record_emotion("u5", "开心").unwrap();
        svc.record_emotion("u5", "喜欢").unwrap();
        svc.record_emotion("u5", "温暖").unwrap();
        svc.record_emotion("u5", "感动").unwrap();

        let trend = svc.get_emotional_trend("u5");
        assert!(trend.contains("积极"));

        // 无历史数据
        assert_eq!(svc.get_emotional_trend("u_empty"), "");
    }

    #[test]
    fn test_update_snapshot_signals() {
        let (svc, _dir) = make_svc();

        let mut signal = HashMap::new();
        signal.insert("relationship_read".to_string(), "用户活跃".to_string());
        signal.insert("emotional_safety".to_string(), "高".to_string());
        signal.insert("tone_guidance".to_string(), "保持友好".to_string());

        let snapshot = svc.update_snapshot_signals("u6", Some(&signal)).unwrap();
        assert!(snapshot.relationship_state_summary.contains("用户活跃"));
        assert!(snapshot
            .relationship_state_summary
            .contains("情绪安全感=高"));
    }

    #[test]
    fn test_refresh_snapshot_prediction_rate() {
        let (svc, _dir) = make_svc();

        // 写入信号
        let mut payload = svc.load_payload("u7");
        payload.stable_signals.push(SignalEntry {
            signal: "prediction:met".to_string(),
            weight: 1,
            created_at: "2024-01-01T00:00:00Z".to_string(),
        });
        payload.stable_signals.push(SignalEntry {
            signal: "prediction:met".to_string(),
            weight: 1,
            created_at: "2024-01-02T00:00:00Z".to_string(),
        });
        payload.stable_signals.push(SignalEntry {
            signal: "prediction:failed".to_string(),
            weight: 1,
            created_at: "2024-01-03T00:00:00Z".to_string(),
        });
        svc.save_payload("u7", &payload);

        let snapshot = svc.refresh_snapshot("u7");
        assert!(snapshot.prediction_feedback_summary.contains("命中率"));
        assert!(snapshot.prediction_feedback_summary.contains("命中2次"));
    }

    #[test]
    fn test_apply_reply_effect_bundle() {
        let (svc, _dir) = make_svc();

        svc.apply_reply_effect_bundle(
            "u8",
            0.9,
            "positive",
            "问候",
            "俏皮",
            "开心",
            "continue",
            Some(true),
        )
        .unwrap();

        // 再应用一次确保情绪趋势有足够数据
        svc.apply_reply_effect_bundle(
            "u8",
            0.7,
            "positive",
            "问候",
            "幽默",
            "喜欢",
            "continue",
            Some(true),
        )
        .unwrap();

        let snapshot = svc.get_snapshot("u8");
        assert!(snapshot.intimacy_level > 0.0);
        assert!(snapshot.explicit_feedback_count > 0);
        assert!(!snapshot.emotional_trend.is_empty());
    }

    #[test]
    fn test_apply_reply_effect_bundle_negative() {
        let (svc, _dir) = make_svc();

        svc.apply_reply_effect_bundle(
            "u9",
            -0.5,
            "negative",
            "回复不当",
            "",
            "生气",
            "clarify",
            Some(false),
        )
        .unwrap();

        let snapshot = svc.get_snapshot("u9");
        // 负向反馈应降低亲密度
        assert!(snapshot.intimacy_level < 0.01);
    }
}
