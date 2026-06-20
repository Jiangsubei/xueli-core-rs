//! DriveEngine — 内驱力系统状态管理核心。
//!
//! 职责：
//!   - 状态读取（消费方接口）
//!   - 事件增量应用
//!   - 反思结果应用
//!   - 自主演化（衰减 tick、夜间恢复）
//!   - 持久化

use std::collections::HashMap;

use chrono::Utc;
use tracing::{debug, info};

use super::event_rules::{DriveEventRuleEngine, WeightAdjustment};
use super::models::{
    DriveContext, DriveSnapshot, MotivationalDimension, MotivationalKey, PADVector,
    ReflectionOutput, RelationalState,
};
use super::store::DriveStore;

/// 内驱力事件类型 — 由外部系统（如 BotRuntime）在消息处理完成后注入。
#[derive(Debug, Clone)]
pub enum DriveEvent {
    /// 消息处理完成事件
    MessageProcessed {
        user_id: String,
        message: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
}

/// 内驱力系统状态管理核心。
pub struct DriveEngine {
    store: DriveStore,
    scope_key: String,
    enabled: bool,
    snapshot: Option<DriveSnapshot>,
    rule_engine: DriveEventRuleEngine,
    /// 当前轮次累积的谨慎度指导
    current_guidance: Vec<String>,
    /// 当前轮次累积的事件模式
    current_event_patterns: Vec<String>,
}

impl DriveEngine {
    pub fn new(store: DriveStore, scope_key: &str, enabled: bool) -> Self {
        Self {
            store,
            scope_key: scope_key.to_string(),
            enabled,
            snapshot: None,
            rule_engine: DriveEventRuleEngine::new(None),
            current_guidance: Vec::new(),
            current_event_patterns: Vec::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn scope_key(&self) -> &str {
        &self.scope_key
    }

    // ─── 生命周期 ───────────────────────────────────────

    /// 从持久化加载状态，不存在则创建默认。
    pub async fn load(&mut self) {
        let snapshot = self.store.load(&self.scope_key).await;
        if let Some(snap) = snapshot {
            if !snap.event_rules.rules.is_empty() {
                self.rule_engine = DriveEventRuleEngine::new(Some(snap.event_rules.clone()));
            }
            self.snapshot = Some(snap);
        } else {
            self.snapshot = Some(DriveSnapshot::create_default(&self.scope_key));
            self.persist().await;
        }
    }

    async fn persist(&mut self) {
        if self.snapshot.is_none() {
            return;
        }
        let now = Utc::now().to_rfc3339();
        if let Some(snap) = self.snapshot.as_mut() {
            snap.updated_at = now;
            snap.event_rules = self.rule_engine.rule_set().clone();
        }
        if let Some(snap) = self.snapshot.as_ref() {
            let _ = self.store.save(&self.scope_key, snap).await;
        }
    }

    fn ensure_snapshot(&mut self) -> &mut DriveSnapshot {
        if self.snapshot.is_none() {
            self.snapshot = Some(DriveSnapshot::create_default(&self.scope_key));
        }
        self.snapshot.as_mut().unwrap()
    }

    fn ensure_motivational(
        snap: &mut DriveSnapshot,
    ) -> &mut HashMap<String, MotivationalDimension> {
        for key in MotivationalKey::all() {
            if !snap.motivational.contains_key(key.as_str()) {
                snap.motivational
                    .insert(key.as_str().to_string(), MotivationalDimension::default());
            }
        }
        &mut snap.motivational
    }

    // ─── 状态读取（消费方接口） ─────────────────────────

    /// 获取情绪层 PAD 向量。
    pub fn get_affective_state(&self) -> PADVector {
        if !self.enabled || self.snapshot.is_none() {
            return PADVector::default();
        }
        self.snapshot.as_ref().unwrap().affective.pad.clamp()
    }

    /// 获取动机层全部维度。
    pub fn get_motivational_state(&self) -> HashMap<String, MotivationalDimension> {
        if !self.enabled || self.snapshot.is_none() {
            return HashMap::new();
        }
        let mut motivational = self.snapshot.as_ref().unwrap().motivational.clone();
        for key in MotivationalKey::all() {
            if !motivational.contains_key(key.as_str()) {
                motivational.insert(key.as_str().to_string(), MotivationalDimension::default());
            }
        }
        motivational
    }

    /// 获取动机层各维度的有效值。
    pub fn get_motivational_effective(&self) -> HashMap<String, f64> {
        self.get_motivational_state()
            .into_iter()
            .map(|(k, v)| (k, v.effective()))
            .collect()
    }

    /// 获取指定用户的关系层状态。
    pub fn get_relational_state(&self, user_id: &str) -> RelationalState {
        if !self.enabled || self.snapshot.is_none() {
            return RelationalState::default();
        }
        self.snapshot
            .as_ref()
            .unwrap()
            .relational
            .get(user_id)
            .cloned()
            .unwrap_or_default()
    }

    /// 获取统一注入用的内驱力上下文。
    pub fn get_drive_context(&self, user_id: &str) -> DriveContext {
        DriveContext {
            affective: self.get_affective_state(),
            motivational: self.get_motivational_effective(),
            relational: self.get_relational_state(user_id),
            caution_guidance: self.current_guidance.clone(),
            active_event_patterns: self.current_event_patterns.clone(),
            memory_context: String::new(),
            scope_key: self.scope_key.clone(),
            user_id: user_id.to_string(),
        }
    }

    /// 获取当前累积的谨慎度指导列表。
    pub fn get_caution_guidance(&self) -> Vec<String> {
        self.current_guidance.clone()
    }

    /// 获取当前累积的活跃事件模式列表。
    pub fn get_active_event_patterns(&self) -> Vec<String> {
        self.current_event_patterns.clone()
    }

    /// 清空当前轮次累积的指导和事件模式（在回复完成后调用）。
    pub fn clear_guidance(&mut self) {
        self.current_guidance.clear();
        self.current_event_patterns.clear();
    }

    // ─── 状态更新（调度器接口） ─────────────────────────

    /// 外部事件入口 — 将 DriveEvent 映射为事件模式并应用增量。
    pub async fn on_event(&mut self, event: DriveEvent) {
        if !self.enabled {
            return;
        }
        let pattern = match event {
            DriveEvent::MessageProcessed { .. } => "message_processed",
        };
        self.apply_event_deltas(&[pattern.to_string()]).await;
    }

    /// 根据事件模式匹配规则，计算并应用瞬时偏移。
    pub async fn apply_event_deltas(&mut self, event_patterns: &[String]) {
        if !self.enabled {
            return;
        }
        let (aff_delta, mot_deltas, guidance_list) =
            self.rule_engine.compute_deltas(event_patterns);
        if aff_delta.is_zero() && mot_deltas.is_empty() {
            return;
        }

        let snap = self.ensure_snapshot();
        Self::ensure_motivational(snap);

        // 情绪层增量
        let pad = &mut snap.affective.pad;
        pad.valence += aff_delta.valence;
        pad.arousal += aff_delta.arousal;
        pad.dominance += aff_delta.dominance;
        snap.affective.pad = pad.clamp();
        snap.affective.updated_at = Utc::now().to_rfc3339();

        // 动机层增量
        for (key, delta) in &mot_deltas {
            if let Some(dim) = snap.motivational.get_mut(key) {
                dim.transient_offset += delta;
                dim.transient_offset = dim.transient_offset.clamp(-1.0, 1.0);
            }
        }

        // 累积指导
        if !guidance_list.is_empty() {
            self.current_guidance.extend(guidance_list);
        }

        // 累积事件模式
        self.current_event_patterns
            .extend(event_patterns.iter().cloned());

        self.persist().await;
        debug!(
            "[DriveEngine] 事件增量应用: patterns={:?} aff=({:.2},{:.2},{:.2}) mot={:?} guidance={}",
            event_patterns,
            aff_delta.valence,
            aff_delta.arousal,
            aff_delta.dominance,
            mot_deltas,
            self.current_guidance.len(),
        );
    }

    /// 应用 LLM 反思输出：更新基线、规则集替换、调整规则权重。
    pub async fn apply_reflection_result(&mut self, result: &ReflectionOutput) {
        if !self.enabled {
            return;
        }
        let snap = self.ensure_snapshot();
        Self::ensure_motivational(snap);

        // 动机层基线更新
        for (key, new_baseline) in &result.baseline_updates {
            if let Some(dim) = snap.motivational.get_mut(key) {
                dim.baseline = new_baseline.clamp(0.0, 1.0);
            }
        }

        // 情绪层基线偏移
        let pad = &mut snap.affective.pad;
        pad.valence += result.affective_baseline_shift.valence;
        pad.arousal += result.affective_baseline_shift.arousal;
        pad.dominance += result.affective_baseline_shift.dominance;
        snap.affective.pad = pad.clamp();

        // 完整规则集替换
        if let Some(ref new_rule_set) = result.new_rule_set {
            self.rule_engine.update_rule_set(new_rule_set.clone());
            info!(
                "[DriveEngine] 规则集替换: version={} rules={}",
                new_rule_set.version,
                new_rule_set.rules.len()
            );
        }

        // 规则权重调整
        if !result.rule_adjustments.is_empty() {
            let adjustments: Vec<WeightAdjustment> = result
                .rule_adjustments
                .iter()
                .map(|adj| WeightAdjustment {
                    rule_id: adj.rule_id.clone(),
                    new_weight: adj.new_weight,
                    new_decay_rate: adj.new_decay_rate,
                    reason: adj.reason.clone(),
                })
                .collect();
            self.rule_engine.apply_weight_adjustments(&adjustments, 0.3);
        }

        self.persist().await;
        info!(
            "[DriveEngine] 反思结果应用: baselines={:?} rule_set_replaced={} summary={}",
            result.baseline_updates,
            result.new_rule_set.is_some(),
            if result.summary.is_empty() {
                ""
            } else {
                &result.summary[..result.summary.len().min(80)]
            }
        );
    }

    // ─── 自主演化 ───────────────────────────────────────

    /// 衰减 tick：情绪层向中性回归 + 动机层瞬时偏移衰减。
    pub async fn decay_tick(&mut self) {
        if !self.enabled || self.snapshot.is_none() {
            return;
        }
        let snap = self.snapshot.as_mut().unwrap();
        Self::ensure_motivational(snap);

        // 情绪层：valence 向 0 回归，arousal 向 0.5 回归，dominance 向 0.5 回归
        let pad = &mut snap.affective.pad;
        pad.valence *= 0.95;
        pad.arousal = 0.5 + (pad.arousal - 0.5) * 0.95;
        pad.dominance = 0.5 + (pad.dominance - 0.5) * 0.95;
        snap.affective.pad = pad.clamp();

        // 动机层：各维度瞬时偏移衰减
        for dim in snap.motivational.values_mut() {
            dim.apply_decay();
        }

        self.persist().await;
    }

    /// 夜间恢复：情绪层向中性回归，能量类维度恢复。
    pub async fn night_recovery(&mut self) {
        if !self.enabled || self.snapshot.is_none() {
            return;
        }
        let snap = self.snapshot.as_mut().unwrap();
        Self::ensure_motivational(snap);

        // 情绪层向中性回归
        let pad = &mut snap.affective.pad;
        pad.valence *= 0.5;
        pad.arousal = 0.5 + (pad.arousal - 0.5) * 0.3;
        pad.dominance = 0.5 + (pad.dominance - 0.5) * 0.3;
        snap.affective.pad = pad.clamp();

        // 动机层：社交需求和主动性恢复
        for key in [MotivationalKey::SocialDrive, MotivationalKey::Proactivity] {
            if let Some(dim) = snap.motivational.get_mut(key.as_str()) {
                // Logistic 增长恢复
                dim.baseline = (dim.baseline + 0.1 * dim.baseline * (1.0 - dim.baseline)).min(1.0);
                dim.transient_offset = 0.0;
            }
        }

        self.persist().await;
        info!("[DriveEngine] 夜间恢复完成: scope={}", self.scope_key);
    }

    // ─── 规则管理 ───────────────────────────────────────

    /// 获取当前完整快照副本（用于反思等需要关系层的场景）。
    pub fn get_snapshot(&self) -> Option<DriveSnapshot> {
        self.snapshot.clone()
    }

    /// 获取当前事件规则集。
    pub fn get_event_rules(&self) -> &super::models::EventRuleSet {
        self.rule_engine.rule_set()
    }

    /// 替换事件规则集。
    pub async fn update_event_rules(&mut self, new_rules: super::models::EventRuleSet) {
        self.rule_engine.update_rule_set(new_rules);
        self.persist().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> DriveStore {
        let dir = tempfile::tempdir().unwrap();
        DriveStore::new(dir.path().to_path_buf())
    }

    #[tokio::test]
    async fn test_load_creates_default() {
        let store = make_store();
        let mut engine = DriveEngine::new(store, "test_scope", true);
        engine.load().await;
        assert!(engine.snapshot.is_some());
        assert_eq!(engine.scope_key(), "test_scope");
    }

    #[tokio::test]
    async fn test_apply_event_deltas() {
        let store = make_store();
        let mut engine = DriveEngine::new(store, "test_scope", true);
        engine.load().await;
        let patterns = vec!["negative_feedback".to_string()];
        engine.apply_event_deltas(&patterns).await;
        let aff = engine.get_affective_state();
        assert!(aff.valence < 0.0);
    }

    #[tokio::test]
    async fn test_decay_tick() {
        let store = make_store();
        let mut engine = DriveEngine::new(store, "test_scope", true);
        engine.load().await;
        // 先应用增量使 valence 偏离 0
        let patterns = vec!["negative_feedback".to_string()];
        engine.apply_event_deltas(&patterns).await;
        let valence_before = engine.get_affective_state().valence;
        engine.decay_tick().await;
        let valence_after = engine.get_affective_state().valence;
        // 衰减后 valence 应更接近 0
        assert!(valence_after.abs() < valence_before.abs());
    }

    #[tokio::test]
    async fn test_night_recovery() {
        let store = make_store();
        let mut engine = DriveEngine::new(store, "test_scope", true);
        engine.load().await;
        engine.night_recovery().await;
        let aff = engine.get_affective_state();
        // 夜间恢复后 valence 应接近 0
        assert!(aff.valence.abs() < 0.01);
    }

    #[tokio::test]
    async fn test_get_drive_context() {
        let store = make_store();
        let mut engine = DriveEngine::new(store, "test_scope", true);
        engine.load().await;
        let ctx = engine.get_drive_context("user1");
        assert_eq!(ctx.scope_key, "test_scope");
        assert_eq!(ctx.user_id, "user1");
        assert!(!ctx.motivational.is_empty());
    }

    #[tokio::test]
    async fn test_disabled_engine() {
        let store = make_store();
        let mut engine = DriveEngine::new(store, "test_scope", false);
        engine.load().await;
        let aff = engine.get_affective_state();
        assert_eq!(aff.valence, 0.0);
        let patterns = vec!["negative_feedback".to_string()];
        engine.apply_event_deltas(&patterns).await;
        // 应无变化
        assert_eq!(engine.get_affective_state().valence, 0.0);
    }

    #[tokio::test]
    async fn test_on_event_message_processed() {
        let store = make_store();
        let mut engine = DriveEngine::new(store, "test_scope", true);
        engine.load().await;
        let event = DriveEvent::MessageProcessed {
            user_id: "user1".to_string(),
            message: "hello".to_string(),
            timestamp: Utc::now(),
        };
        engine.on_event(event).await;
        // message_processed 无匹配规则，状态应不变
        let aff = engine.get_affective_state();
        assert_eq!(aff.valence, 0.0);
    }

    #[tokio::test]
    async fn test_on_event_disabled() {
        let store = make_store();
        let mut engine = DriveEngine::new(store, "test_scope", false);
        engine.load().await;
        let event = DriveEvent::MessageProcessed {
            user_id: "user1".to_string(),
            message: "hello".to_string(),
            timestamp: Utc::now(),
        };
        engine.on_event(event).await;
        // 禁用状态下应无变化
        let aff = engine.get_affective_state();
        assert_eq!(aff.valence, 0.0);
    }
}
