//! 事件增量规则集管理 — 支持规则匹配、权重调整和 LLM 自适应。

use std::collections::HashMap;

use tracing::debug;

use super::models::{EventRule, EventRuleSet, PADVector};

// ─── 默认规则集 ─────────────────────────────────────────

/// 构建默认事件增量规则集。
pub fn build_default_rule_set() -> EventRuleSet {
    let rules = vec![
        EventRule {
            rule_id: "negative_feedback".into(),
            event_pattern: "negative_feedback".into(),
            affective_deltas: PADVector {
                valence: -0.15,
                arousal: 0.1,
                dominance: -0.05,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.2);
                m.insert("social_drive".into(), -0.1);
                m
            },
            weight: 1.0,
            guidance: "有负面反馈记录，语气放轻、避免过度主动。".into(),
            description: "用户负面反馈：降低愉悦度，提高谨慎".into(),
        },
        EventRule {
            rule_id: "positive_feedback".into(),
            event_pattern: "positive_feedback".into(),
            affective_deltas: PADVector {
                valence: 0.1,
                arousal: 0.05,
                dominance: 0.05,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("social_drive".into(), 0.1);
                m.insert("proactivity".into(), 0.05);
                m.insert("expressiveness".into(), 0.05);
                m
            },
            weight: 1.0,
            guidance: String::new(),
            description: "用户正面反馈：提升愉悦度，增强社交需求".into(),
        },
        EventRule {
            rule_id: "privacy_sensitive".into(),
            event_pattern: "privacy_sensitive".into(),
            affective_deltas: PADVector {
                valence: -0.05,
                arousal: 0.15,
                dominance: -0.1,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.4);
                m
            },
            weight: 1.0,
            guidance: "涉及隐私敏感内容，保持克制。".into(),
            description: "隐私敏感词触发：大幅提高谨慎度".into(),
        },
        EventRule {
            rule_id: "long_silence_resume".into(),
            event_pattern: "long_silence_resume".into(),
            affective_deltas: PADVector {
                valence: 0.05,
                arousal: 0.1,
                dominance: 0.0,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("social_drive".into(), 0.15);
                m.insert("curiosity".into(), 0.1);
                m
            },
            weight: 1.0,
            guidance: String::new(),
            description: "长时间沉默后首次互动：提升社交需求和好奇".into(),
        },
        EventRule {
            rule_id: "emotional_volatility".into(),
            event_pattern: "emotional_volatility".into(),
            affective_deltas: PADVector {
                valence: -0.1,
                arousal: 0.2,
                dominance: -0.1,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.15);
                m.insert("social_drive".into(), -0.05);
                m
            },
            weight: 1.0,
            guidance: String::new(),
            description: "用户情绪剧烈波动：提高唤醒和谨慎".into(),
        },
        EventRule {
            rule_id: "conflict_detected".into(),
            event_pattern: "conflict_detected".into(),
            affective_deltas: PADVector {
                valence: -0.2,
                arousal: 0.15,
                dominance: -0.15,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.3);
                m.insert("social_drive".into(), -0.15);
                m.insert("belonging".into(), -0.1);
                m
            },
            weight: 1.0,
            guidance: String::new(),
            description: "检测到冲突：大幅降低愉悦度，提高谨慎".into(),
        },
        // ─── 收编自原 _build_caution_signal 的规则 ───
        EventRule {
            rule_id: "memory_uncertain".into(),
            event_pattern: "memory_uncertain".into(),
            affective_deltas: PADVector {
                valence: -0.05,
                arousal: 0.05,
                dominance: -0.05,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.25);
                m
            },
            weight: 1.0,
            guidance: "涉及记忆或事实时不要说死，必要时先确认。".into(),
            description: "记忆不确定性：提高谨慎度".into(),
        },
        EventRule {
            rule_id: "memory_context_empty".into(),
            event_pattern: "memory_context_empty".into(),
            affective_deltas: PADVector {
                valence: 0.0,
                arousal: 0.0,
                dominance: -0.05,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.2);
                m
            },
            weight: 1.0,
            guidance: "没有可靠记忆依据时，避免假装记得细节。".into(),
            description: "记忆上下文为空：提高谨慎度".into(),
        },
        EventRule {
            rule_id: "low_emotional_safety".into(),
            event_pattern: "low_emotional_safety".into(),
            affective_deltas: PADVector {
                valence: -0.1,
                arousal: 0.1,
                dominance: -0.1,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.3);
                m.insert("expressiveness".into(), -0.1);
                m
            },
            weight: 1.0,
            guidance: "语气放轻，先保证理解准确。".into(),
            description: "用户情绪安全感低：大幅提高谨慎，降低表达欲".into(),
        },
        EventRule {
            rule_id: "risk_posture_careful".into(),
            event_pattern: "risk_posture_careful".into(),
            affective_deltas: PADVector {
                valence: 0.0,
                arousal: 0.0,
                dominance: -0.05,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.15);
                m
            },
            weight: 1.0,
            guidance: "保持克制，避免过度推断。".into(),
            description: "风险姿态为谨慎：提高谨慎度".into(),
        },
        EventRule {
            rule_id: "high_interruption_risk".into(),
            event_pattern: "high_interruption_risk".into(),
            affective_deltas: PADVector {
                valence: 0.0,
                arousal: 0.05,
                dominance: -0.05,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.25);
                m.insert("expressiveness".into(), -0.15);
                m
            },
            weight: 1.0,
            guidance: "群聊插话风险较高，回复要短且不打断。".into(),
            description: "群聊插话风险高：提高谨慎，降低表达欲".into(),
        },
        EventRule {
            rule_id: "vision_partial_failure".into(),
            event_pattern: "vision_partial_failure".into(),
            affective_deltas: PADVector {
                valence: 0.0,
                arousal: 0.0,
                dominance: -0.05,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.15);
                m
            },
            weight: 1.0,
            guidance: "图片信息不完整时，不要断言图片细节。".into(),
            description: "图片识别部分失败：提高谨慎度".into(),
        },
        EventRule {
            rule_id: "negative_feedback_history".into(),
            event_pattern: "negative_feedback_history".into(),
            affective_deltas: PADVector {
                valence: -0.1,
                arousal: 0.05,
                dominance: -0.1,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("caution".into(), 0.3);
                m.insert("expressiveness".into(), -0.1);
                m
            },
            weight: 1.0,
            guidance: "有负面反馈记录，语气放轻、避免过度主动。".into(),
            description: "历史负面反馈：大幅提高谨慎，降低表达欲".into(),
        },
        // ─── 收编自 TemporalContext 的桥接规则 ───
        EventRule {
            rule_id: "same_day_resume".into(),
            event_pattern: "same_day_resume".into(),
            affective_deltas: PADVector {
                valence: 0.03,
                arousal: 0.05,
                dominance: 0.0,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("social_drive".into(), 0.05);
                m.insert("proactivity".into(), 0.05);
                m
            },
            weight: 1.0,
            guidance: String::new(),
            description: "当天恢复对话：微增社交需求和主动性".into(),
        },
        EventRule {
            rule_id: "short_resume".into(),
            event_pattern: "short_resume".into(),
            affective_deltas: PADVector {
                valence: 0.05,
                arousal: 0.08,
                dominance: 0.0,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("social_drive".into(), 0.1);
                m.insert("curiosity".into(), 0.05);
                m.insert("proactivity".into(), 0.05);
                m
            },
            weight: 1.0,
            guidance: String::new(),
            description: "短期恢复对话：增加社交需求和好奇".into(),
        },
        EventRule {
            rule_id: "stale_resume".into(),
            event_pattern: "stale_resume".into(),
            affective_deltas: PADVector {
                valence: 0.05,
                arousal: 0.1,
                dominance: 0.0,
            },
            motivational_deltas: {
                let mut m = HashMap::new();
                m.insert("social_drive".into(), 0.15);
                m.insert("curiosity".into(), 0.1);
                m.insert("proactivity".into(), 0.05);
                m.insert("belonging".into(), -0.05);
                m
            },
            weight: 1.0,
            guidance: String::new(),
            description: "长期沉默后恢复：增加社交需求，归属感略降".into(),
        },
    ];

    EventRuleSet { rules, version: 1 }
}

// ─── 权重调整参数 ───────────────────────────────────────

/// 权重调整建议
#[derive(Debug, Clone)]
pub struct WeightAdjustment {
    pub rule_id: String,
    pub new_weight: Option<f64>,
    pub new_decay_rate: Option<f64>,
    pub reason: String,
}

// ─── 事件增量规则引擎 ───────────────────────────────────

/// 事件增量规则引擎：匹配事件 → 计算瞬时偏移。
pub struct DriveEventRuleEngine {
    rule_set: EventRuleSet,
}

impl DriveEventRuleEngine {
    pub fn new(rule_set: Option<EventRuleSet>) -> Self {
        Self {
            rule_set: rule_set.unwrap_or_else(build_default_rule_set),
        }
    }

    /// 获取当前规则集的引用
    pub fn rule_set(&self) -> &EventRuleSet {
        &self.rule_set
    }

    /// 替换规则集（由反思结果驱动）。
    pub fn update_rule_set(&mut self, new_rules: EventRuleSet) {
        self.rule_set = new_rules;
    }

    /// 根据反思输出的权重调整建议，更新规则权重和衰减率。
    pub fn apply_weight_adjustments(
        &mut self,
        adjustments: &[WeightAdjustment],
        max_adjustment: f64,
    ) {
        for adj in adjustments {
            let rule = match self
                .rule_set
                .rules
                .iter_mut()
                .find(|r| r.rule_id == adj.rule_id)
            {
                Some(r) => r,
                None => continue,
            };

            if let Some(new_weight) = adj.new_weight {
                let delta = new_weight - rule.weight;
                let clamped_delta = delta.clamp(-max_adjustment, max_adjustment);
                rule.weight = (rule.weight + clamped_delta).clamp(0.0, 2.0);
            }

            if let Some(new_decay) = adj.new_decay_rate {
                debug!(
                    "[规则引擎] 衰减率调整建议: rule={} decay={}",
                    adj.rule_id, new_decay
                );
            }
        }

        self.rule_set.version += 1;
    }

    /// 对一组事件模式匹配规则，汇总情绪层和动机层增量及指导。
    ///
    /// 返回 (affective_delta, motivational_deltas, guidance_list)
    pub fn compute_deltas(
        &self,
        event_patterns: &[String],
    ) -> (PADVector, HashMap<String, f64>, Vec<String>) {
        let mut aff_delta = PADVector {
            valence: 0.0,
            arousal: 0.0,
            dominance: 0.0,
        };
        let mut mot_deltas: HashMap<String, f64> = HashMap::new();
        let mut guidance_list: Vec<String> = Vec::new();

        for pattern in event_patterns {
            for rule in self.rule_set.match_pattern(pattern) {
                let w = rule.weight;
                aff_delta.valence += rule.affective_deltas.valence * w;
                aff_delta.arousal += rule.affective_deltas.arousal * w;
                aff_delta.dominance += rule.affective_deltas.dominance * w;
                for (key, delta) in &rule.motivational_deltas {
                    *mot_deltas.entry(key.clone()).or_insert(0.0) += delta * w;
                }
                if !rule.guidance.is_empty() {
                    guidance_list.push(rule.guidance.clone());
                }
            }
        }

        (aff_delta, mot_deltas, guidance_list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_default_rule_set() {
        let rs = build_default_rule_set();
        assert!(!rs.rules.is_empty());
        assert_eq!(rs.version, 1);
    }

    #[test]
    fn test_compute_deltas_negative_feedback() {
        let engine = DriveEventRuleEngine::new(None);
        let patterns = vec!["negative_feedback".to_string()];
        let (aff, mot, guidance) = engine.compute_deltas(&patterns);
        assert!(aff.valence < 0.0);
        assert!(mot.get("caution").copied().unwrap_or(0.0) > 0.0);
        assert!(!guidance.is_empty());
    }

    #[test]
    fn test_compute_deltas_empty_patterns() {
        let engine = DriveEventRuleEngine::new(None);
        let patterns: Vec<String> = Vec::new();
        let (aff, mot, guidance) = engine.compute_deltas(&patterns);
        assert!(aff.is_zero());
        assert!(mot.is_empty());
        assert!(guidance.is_empty());
    }

    #[test]
    fn test_apply_weight_adjustments() {
        let mut engine = DriveEventRuleEngine::new(None);
        let adjustments = vec![WeightAdjustment {
            rule_id: "negative_feedback".into(),
            new_weight: Some(1.5),
            new_decay_rate: None,
            reason: "test".into(),
        }];
        engine.apply_weight_adjustments(&adjustments, 0.3);
        let rule = engine
            .rule_set()
            .rules
            .iter()
            .find(|r| r.rule_id == "negative_feedback")
            .unwrap();
        // 原始 weight=1.0, delta=0.5, clamped to 0.3, result=1.3
        assert!((rule.weight - 1.3).abs() < 0.001);
    }
}
