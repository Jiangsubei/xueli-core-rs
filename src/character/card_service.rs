use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::character::{build_scope_payload_path, legacy_payload_path};
use crate::core::config::{CharacterGrowthConfig, IntimacyThresholdConfig};
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersonaHints {
    pub core_traits: Vec<String>,
    pub tone_preferences: Vec<String>,
    pub bot_persona_hints: Vec<String>,
    #[serde(default)]
    pub behavior_habits: Vec<String>,
    #[serde(default)]
    pub style_adaptation_summary: String,
    #[serde(default)]
    pub relationship_tone_hint: String,
    #[serde(default)]
    pub emotional_trend: String,
    #[serde(default)]
    pub feedback_summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackCategory {
    Praise,
    Criticism,
    Correction,
    Preference,
    Other,
}

impl FeedbackCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            FeedbackCategory::Praise => "praise",
            FeedbackCategory::Criticism => "criticism",
            FeedbackCategory::Correction => "correction",
            FeedbackCategory::Preference => "preference",
            FeedbackCategory::Other => "other",
        }
    }
}

#[async_trait]
pub trait PersonaHintProvider: Send + Sync {
    async fn compute_character_adaptation_signal(&self, payload: &Value) -> Value;
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
pub struct RelationshipProfileInternal {
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
    fn resolve_stage_with_thresholds(intimacy: f64, thresholds: &IntimacyThresholdConfig) -> String {
        if intimacy >= thresholds.trusted {
            "intimate"
        } else if intimacy >= thresholds.close_friend {
            "close_friend"
        } else if intimacy >= thresholds.friend {
            "friend"
        } else if intimacy >= thresholds.acquaintance {
            "acquaintance"
        } else if intimacy >= 0.1 {
            "met_before"
        } else {
            "stranger"
        }
        .into()
    }

    fn resolve_stage(intimacy: f64) -> String {
        Self::resolve_stage_with_thresholds(intimacy, &IntimacyThresholdConfig::default())
    }

    fn update_stage(&mut self) {
        self.relationship_stage = Self::resolve_stage(self.intimacy_level);
    }

    fn update_stage_with_thresholds(&mut self, thresholds: &IntimacyThresholdConfig) {
        self.relationship_stage = Self::resolve_stage_with_thresholds(self.intimacy_level, thresholds);
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
    card: CharacterCard,
    snapshots: Mutex<HashMap<String, CharacterCardSnapshot>>,
    storage_dir: String,
    signal_orchestrator: Option<Arc<dyn PersonaHintProvider>>,
    persona_hints_cache: Mutex<HashMap<String, PersonaHints>>,
    config: CharacterGrowthConfig,
    adaptation_last_update: Mutex<HashMap<String, Instant>>,
    adaptation_last_signal_count: Mutex<HashMap<String, i32>>,
}

impl CharacterCardService {
    pub fn new(card: CharacterCard, storage_dir: &str) -> Self {
        let mut svc = Self {
            card,
            snapshots: Mutex::new(HashMap::new()),
            storage_dir: storage_dir.to_string(),
            signal_orchestrator: None,
            persona_hints_cache: Mutex::new(HashMap::new()),
            config: CharacterGrowthConfig::default(),
            adaptation_last_update: Mutex::new(HashMap::new()),
            adaptation_last_signal_count: Mutex::new(HashMap::new()),
        };
        svc.load_all();
        svc
    }

    pub fn with_config(mut self, config: CharacterGrowthConfig) -> Self {
        self.config = config;
        self
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

    pub fn set_signal_orchestrator(&mut self, orchestrator: Option<Arc<dyn PersonaHintProvider>>) {
        self.signal_orchestrator = orchestrator;
    }

    pub fn get_card(&self) -> &CharacterCard {
        &self.card
    }

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

    pub fn update_snapshot(&self, snapshot: CharacterCardSnapshot) {
        let mut snapshots = self.snapshots.lock().unwrap();
        snapshots.insert(snapshot.user_id.clone(), snapshot.clone());
        self.save_one(&snapshot);
    }

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

    pub fn record_explicit_feedback_category(
        &self,
        user_id: &str,
        category: &str,
    ) -> XueliResult<()> {
        if !self.config.enabled {
            return Ok(());
        }
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

    pub fn record_interaction_signal(&self, user_id: &str, signal_name: &str) -> XueliResult<()> {
        self.record_interaction_signal_weighted(user_id, signal_name, 1)
    }

    pub fn record_interaction_signal_weighted(
        &self,
        user_id: &str,
        signal_name: &str,
        weight: i32,
    ) -> XueliResult<()> {
        if !self.config.enabled {
            return Ok(());
        }
        let normalized = signal_name.trim();
        if normalized.is_empty() {
            return Ok(());
        }
        let mut payload = self.load_payload(user_id);
        payload.stable_signals.push(SignalEntry {
            signal: normalized.to_string(),
            weight: weight.max(1),
            created_at: chrono::Utc::now().to_rfc3339(),
        });
        self.save_payload(user_id, &payload);
        Ok(())
    }

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

    /// 更新用户亲密度并返回关系画像
    pub fn update_intimacy(
        &self,
        user_id: &str,
        delta: f64,
        is_friction: bool,
    ) -> RelationshipProfileInternal {
        if !self.config.relationship_tracking_enabled {
            return RelationshipProfileInternal::default();
        }
        let mut profile = self.get_relationship_profile(user_id);
        let previous_stage = if profile.relationship_stage.is_empty() {
            "stranger".to_string()
        } else {
            profile.relationship_stage.clone()
        };
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
        self.save_relationship_profile(user_id, &profile);
        let current_stage = profile.relationship_stage.clone();
        if previous_stage != current_stage {
            tracing::info!(
                "[关系] 用户 {} 关系阶段变更: {} → {} (亲密度{:.2} 互动{}次)",
                user_id,
                previous_stage,
                current_stage,
                profile.intimacy_level,
                profile.total_interactions,
            );
        }
        profile
    }

    /// 获取用户的关系画像
    pub fn get_relationship_profile(&self, user_id: &str) -> RelationshipProfileInternal {
        let payload = self.load_payload(user_id);
        payload.relationship_profile.unwrap_or_default()
    }

    /// 保存关系画像
    pub fn save_relationship_profile(
        &self,
        user_id: &str,
        profile: &RelationshipProfileInternal,
    ) {
        if !self.config.relationship_tracking_enabled {
            return;
        }
        let mut payload = self.load_payload(user_id);
        let mut profile = profile.clone();
        profile.update_stage_with_thresholds(&self.config.intimacy_thresholds);
        profile.last_intimacy_change = chrono::Utc::now().to_rfc3339();
        payload.relationship_profile = Some(profile);
        self.save_payload(user_id, &payload);
    }

    /// 归一化预期效果值到允许的集合 {continue, satisfy, cool_down, clarify, none}
    pub fn normalize_expected_effect(raw_effect: &str, allow_none: bool) -> String {
        let allowed: &[&str] = if allow_none {
            &["continue", "satisfy", "cool_down", "clarify", "none"]
        } else {
            &["continue", "satisfy", "cool_down", "clarify"]
        };
        let normalized = raw_effect.trim().to_lowercase();
        if allowed.contains(&normalized.as_str()) {
            normalized
        } else {
            String::new()
        }
    }

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

    pub fn refresh_snapshot(&self, user_id: &str) -> CharacterCardSnapshot {
        let mut payload = self.load_payload(user_id);

        let mut category_counts: HashMap<String, usize> = HashMap::new();
        for entry in &payload.explicit_feedback {
            *category_counts.entry(entry.category.clone()).or_insert(0) += 1;
        }

        let mut signal_counts: HashMap<String, i32> = HashMap::new();
        for entry in &payload.stable_signals {
            *signal_counts.entry(entry.signal.clone()).or_insert(0) += entry.weight;
        }

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

        let hints = {
            let cache = self.persona_hints_cache.lock().unwrap();
            cache.get(user_id).cloned().unwrap_or_default()
        };

        let snapshot = CharacterCardSnapshot {
            user_id: user_id.to_string(),
            core_traits: hints.core_traits,
            tone_preferences: hints.tone_preferences,
            bot_persona_hints: hints.bot_persona_hints,
            explicit_feedback_count: category_counts.values().sum(),
            stable_signal_count: signal_counts.values().sum::<i32>() as usize,
            intimacy_level: profile.intimacy_level,
            relationship_stage: if profile.relationship_stage.is_empty() {
                Self::resolve_stage(profile.intimacy_level)
            } else {
                profile.relationship_stage
            },
            emotional_trend: if hints.emotional_trend.is_empty() {
                self.get_emotional_trend(user_id)
            } else {
                hints.emotional_trend
            },
            updated_at: chrono::Utc::now().to_rfc3339(),
            prediction_feedback_summary,
            relationship_tone_hint: hints.relationship_tone_hint,
            behavior_habits: hints.behavior_habits,
            relationship_state_summary: String::new(),
        };

        payload.snapshot = Some(snapshot.clone());
        self.save_payload(user_id, &payload);
        self.update_snapshot(snapshot.clone());

        snapshot
    }

    pub async fn refresh_persona_hints_async(&self, user_id: &str) {
        let orchestrator = match &self.signal_orchestrator {
            Some(o) => o.clone(),
            None => return,
        };

        // Incremental signal detection: skip if signal count delta < 3 and within 300s cooldown
        {
            let last_total = self
                .adaptation_last_signal_count
                .lock()
                .unwrap()
                .get(user_id)
                .copied()
                .unwrap_or(0);
            let last_time = self
                .adaptation_last_update
                .lock()
                .unwrap()
                .get(user_id)
                .copied();
            let elapsed = last_time.map_or(f64::MAX, |t| t.elapsed().as_secs_f64());
            if elapsed < 300.0 {
                let current_total = self.get_total_raw_signals(user_id);
                if current_total - last_total < 3 {
                    return;
                }
            }
        }

        let payload = self.build_adaptation_payload(user_id);
        let result = orchestrator
            .compute_character_adaptation_signal(&payload)
            .await;

        if result.is_null() || result.as_object().map_or(true, |o| o.is_empty()) {
            return;
        }

        let core_traits = list_val(&result, "core_traits", 8);
        let tone_preferences = list_val(&result, "tone_preferences", 8);
        let bot_persona_hints = list_val(&result, "bot_persona_hints", 8);
        let behavior_habits = list_val(&result, "behavior_habits", 8);
        let style_adaptation_summary = str_val(&result, "style_adaptation_summary");
        let relationship_tone_hint = str_val(&result, "relationship_tone_hint");
        let emotional_trend = str_val(&result, "emotional_trend");
        let feedback_summary = str_val(&result, "feedback_summary");

        let hints = PersonaHints {
            core_traits,
            tone_preferences,
            bot_persona_hints,
            behavior_habits,
            style_adaptation_summary,
            relationship_tone_hint,
            emotional_trend,
            feedback_summary,
        };

        {
            let mut cache = self.persona_hints_cache.lock().unwrap();
            cache.insert(user_id.to_string(), hints);
        }

        // Update adaptation tracking
        {
            let current_total = self.get_total_raw_signals(user_id);
            let mut counts = self.adaptation_last_signal_count.lock().unwrap();
            counts.insert(user_id.to_string(), current_total);
        }
        {
            let mut times = self.adaptation_last_update.lock().unwrap();
            times.insert(user_id.to_string(), Instant::now());
        }
    }

    fn get_total_raw_signals(&self, user_id: &str) -> i32 {
        let payload = self.load_payload(user_id);
        (payload.stable_signals.len() + payload.explicit_feedback.len()) as i32
    }

    pub fn get_cached_persona_hints(&self, user_id: &str) -> Option<PersonaHints> {
        let cache = self.persona_hints_cache.lock().unwrap();
        cache.get(user_id).cloned()
    }

    fn build_adaptation_payload(&self, user_id: &str) -> Value {
        let payload = self.load_payload(user_id);
        let profile = payload.relationship_profile.clone().unwrap_or_default();

        let feedback_limit = 50;
        let feedback_slice: Vec<&FeedbackEntry> = payload
            .explicit_feedback
            .iter()
            .rev()
            .take(feedback_limit)
            .collect();
        let feedback: Vec<Value> = feedback_slice
            .iter()
            .rev()
            .map(|e| {
                serde_json::json!({
                    "text": e.text,
                    "category": e.category,
                    "created_at": e.created_at,
                })
            })
            .collect();

        let signals: Vec<Value> = payload
            .stable_signals
            .iter()
            .map(|s| {
                serde_json::json!({
                    "signal": s.signal,
                    "weight": s.weight,
                    "created_at": s.created_at,
                })
            })
            .collect();

        let emotions: Vec<Value> = payload
            .emotional_history
            .iter()
            .map(|e| {
                serde_json::json!({
                    "tone": e.tone,
                    "created_at": e.created_at,
                })
            })
            .collect();

        serde_json::json!({
            "user_id": user_id,
            "explicit_feedback": feedback,
            "stable_signals": signals,
            "relationship_profile": {
                "intimacy_level": profile.intimacy_level,
                "total_interactions": profile.total_interactions,
                "reply_positive_count": profile.reply_positive_count,
                "reply_negative_count": profile.reply_negative_count,
                "reply_repair_count": profile.reply_repair_count,
                "friction_signals": profile.friction_signals,
                "last_feedback_label": profile.last_feedback_label,
                "last_reply_intent": profile.last_reply_intent,
                "relationship_stage": profile.relationship_stage,
                "last_interaction_at": profile.last_interaction_at,
                "last_intimacy_change": profile.last_intimacy_change,
            },
            "emotional_history": emotions,
        })
    }

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
        if !self.config.enabled {
            return Ok(());
        }
        let mut payload = self.load_payload(user_id);
        let now = chrono::Utc::now().to_rfc3339();

        let normalized_feedback = feedback_label.trim().to_lowercase();
        let normalized_intent = intent_label.trim().to_lowercase();
        let score_label = if score > 0.3 {
            "positive"
        } else if score < -0.3 {
            "negative"
        } else if score < 0.0 {
            "repair"
        } else {
            "neutral"
        };

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

        if !normalized_intent.is_empty() && !normalized_feedback.is_empty() {
            payload.stable_signals.push(SignalEntry {
                signal: format!("reply_effect:{}:{}", normalized_intent, normalized_feedback),
                weight: 1,
                created_at: now.clone(),
            });
        }

        let normalized_expected = Self::normalize_expected_effect(expected_effect, false);
        if !normalized_expected.is_empty() {
            if let Some(met) = prediction_met {
                let suffix = if met { "met" } else { "failed" };
                payload.stable_signals.push(SignalEntry {
                    signal: format!("expected_effect:{}:{}", normalized_expected, suffix),
                    weight: 1,
                    created_at: now.clone(),
                });
            }
            let actual = Self::normalize_expected_effect(
                if score > 0.0 { "satisfy" } else { "clarify" },
                true,
            );
            if !actual.is_empty() {
                payload.stable_signals.push(SignalEntry {
                    signal: format!("expected_actual:{}:{}", normalized_expected, actual),
                    weight: 1,
                    created_at: now.clone(),
                });
            }
        }

        if let Some(met) = prediction_met {
            let suffix = if met { "met" } else { "failed" };
            payload.stable_signals.push(SignalEntry {
                signal: format!("prediction:{}", suffix),
                weight: 1,
                created_at: now.clone(),
            });
        }

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

        if !style_label.trim().is_empty() {
            payload.explicit_feedback.push(FeedbackEntry {
                text: "[llm feedback triage]".to_string(),
                category: style_label.trim().to_string(),
                created_at: now.clone(),
            });
        }

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

    pub fn classify_feedback(&self, feedback_text: &str) -> FeedbackCategory {
        let text = feedback_text.to_lowercase();

        let praise_keywords = [
            "好", "棒", "赞", "厉害", "优秀", "不错", "喜欢", "爱了", "good", "great", "nice",
            "感谢", "谢谢", "太强", "牛逼", "牛", "厉害", "真好",
        ];
        let criticism_keywords = [
            "不好", "差", "烂", "糟糕", "失望", "讨厌", "烦", "bad", "terrible", "垃圾", "无语",
            "不行",
        ];
        let correction_keywords = [
            "不对",
            "错了",
            "纠正",
            "更正",
            "应该是",
            "不是",
            "改一下",
            "说错了",
            "理解错了",
            "误解",
        ];
        let preference_keywords = [
            "希望",
            "想要",
            "更喜欢",
            "偏好",
            "倾向",
            "喜欢...风格",
            "能不能",
            "可以...吗",
            "以后",
            "下次",
        ];

        if praise_keywords.iter().any(|k| text.contains(k)) {
            FeedbackCategory::Praise
        } else if criticism_keywords.iter().any(|k| text.contains(k)) {
            FeedbackCategory::Criticism
        } else if correction_keywords.iter().any(|k| text.contains(k)) {
            FeedbackCategory::Correction
        } else if preference_keywords.iter().any(|k| text.contains(k)) {
            FeedbackCategory::Preference
        } else {
            FeedbackCategory::Other
        }
    }

    // ── async variants ──

    pub async fn record_interaction_signal_async(
        &self,
        user_id: String,
        signal_name: String,
    ) -> XueliResult<()> {
        tokio::task::block_in_place(|| self.record_interaction_signal(&user_id, &signal_name))
    }

    pub async fn record_feedback_async(
        &self,
        user_id: String,
        sentiment: f64,
        traits: Vec<String>,
        preferences: Vec<String>,
    ) {
        let traits_refs: Vec<&str> = traits.iter().map(|s| s.as_str()).collect();
        let prefs_refs: Vec<&str> = preferences.iter().map(|s| s.as_str()).collect();
        tokio::task::block_in_place(|| {
            self.record_feedback(&user_id, sentiment, &traits_refs, &prefs_refs)
        })
    }

    pub async fn refresh_snapshot_async(&self, user_id: String) -> CharacterCardSnapshot {
        tokio::task::block_in_place(|| self.refresh_snapshot(&user_id))
    }

    // ── 载荷管理 ──

    fn load_payload(&self, user_id: &str) -> UserPayload {
        let path = build_scope_payload_path(&self.storage_dir, user_id, "payload.json");
        match std::fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str(&data) {
                Ok(p) => return p,
                Err(_) => {}
            },
            Err(_) => {}
        }
        let legacy = legacy_payload_path(&self.storage_dir, user_id, "payload.json");
        match std::fs::read_to_string(&legacy) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => UserPayload::default(),
        }
    }

    fn save_payload(&self, user_id: &str, payload: &UserPayload) {
        let path = build_scope_payload_path(&self.storage_dir, user_id, "payload.json");
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content = serde_json::to_string_pretty(payload).unwrap_or_default();
        let tmp = format!("{}.tmp", path);
        let _ = std::fs::write(&tmp, &content);
        let _ = std::fs::rename(&tmp, &path);
    }

    // ── 快照文件读写 ──

    fn save_one(&self, snapshot: &CharacterCardSnapshot) {
        let path = build_scope_payload_path(&self.storage_dir, &snapshot.user_id, "card.json");
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
                if path.extension().map_or(false, |e| e == "json")
                    && path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map_or(false, |n| n.ends_with("_card.json"))
                {
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

// ── helpers for Value extraction ──

fn str_val(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn list_val(v: &Value, key: &str, _max_len: usize) -> Vec<String> {
    v.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
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
        assert_eq!(snapshot.stable_signal_count, 0);
    }

    #[test]
    fn test_record_emotion_sliding_window() {
        let (svc, _dir) = make_svc();

        for i in 0..12 {
            svc.record_emotion("u4", &format!("emotion_{}", i)).unwrap();
        }

        let trend = svc.get_emotional_trend("u4");
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
        assert!(snapshot.intimacy_level < 0.01);
    }

    #[test]
    fn test_classify_feedback() {
        let (svc, _dir) = make_svc();
        assert_eq!(svc.classify_feedback("你太棒了"), FeedbackCategory::Praise);
        assert_eq!(
            svc.classify_feedback("这不对，你理解错了"),
            FeedbackCategory::Correction
        );
        assert_eq!(
            svc.classify_feedback("希望你以后能更活跃"),
            FeedbackCategory::Preference
        );
        assert_eq!(svc.classify_feedback("随便聊聊"), FeedbackCategory::Other);
    }

    #[test]
    fn test_normalize_expected_effect() {
        assert_eq!(
            CharacterCardService::normalize_expected_effect("continue", false),
            "continue"
        );
        assert_eq!(
            CharacterCardService::normalize_expected_effect("satisfy", false),
            "satisfy"
        );
        assert_eq!(
            CharacterCardService::normalize_expected_effect("cool_down", false),
            "cool_down"
        );
        assert_eq!(
            CharacterCardService::normalize_expected_effect("clarify", false),
            "clarify"
        );
        assert_eq!(
            CharacterCardService::normalize_expected_effect("none", false),
            ""
        );
        assert_eq!(
            CharacterCardService::normalize_expected_effect("none", true),
            "none"
        );
        assert_eq!(
            CharacterCardService::normalize_expected_effect("unknown", false),
            ""
        );
    }

    #[test]
    fn test_persona_hints_default() {
        let hints = PersonaHints::default();
        assert!(hints.core_traits.is_empty());
        assert!(hints.tone_preferences.is_empty());
    }

    #[test]
    fn test_get_cached_persona_hints_none() {
        let (svc, _dir) = make_svc();
        assert!(svc.get_cached_persona_hints("u_none").is_none());
    }

    #[test]
    fn test_scope_based_payload_path_group() {
        let dir = "/tmp/test_cards";
        let user_id = "qq:group:g123:u456";
        let path = build_scope_payload_path(dir, user_id, "payload.json");
        assert!(path.contains("qq_group_g123_u456"));
        assert!(path.contains("payload.json"));
    }

    #[test]
    fn test_scope_based_payload_path_private() {
        let dir = "/tmp/test_cards";
        let user_id = "qq:private:u789";
        let path = build_scope_payload_path(dir, user_id, "payload.json");
        assert!(path.contains("qq_private_u789"));
        assert!(path.contains("payload.json"));
    }

    #[test]
    fn test_scope_based_payload_path_legacy_fallback() {
        let dir = "/tmp/test_cards";
        let user_id = "plain_user";
        let path = build_scope_payload_path(dir, user_id, "payload.json");
        assert!(path.contains("plain_user_payload.json"));
    }
}
