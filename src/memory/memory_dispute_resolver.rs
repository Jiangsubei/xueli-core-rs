use crate::core::config::MemoryDisputeConfig;

/// 记忆冲突解决判定结果
///
/// 对应 Python 版 `src.core.models.MemoryDisputeDecision`
#[derive(Debug, Clone)]
pub struct MemoryDisputeDecision {
    /// 置信度等级：high_confidence / normal / ignore
    pub level: String,
    /// 置信度分数 0.0-1.0
    pub confidence: f64,
    /// 待执行动作
    pub action: String,
    /// 冲突类型
    pub conflict_type: String,
    /// 冲突简述
    pub summary: String,
    /// 决策理由
    pub reason: String,
    /// 受影响的目标记忆 ID 列表
    pub targets: Vec<String>,
    /// 支持证据
    pub evidence: Vec<String>,
}

impl Default for MemoryDisputeDecision {
    fn default() -> Self {
        Self {
            level: "ignore".to_string(),
            confidence: 0.0,
            action: String::new(),
            conflict_type: "none".to_string(),
            summary: String::new(),
            reason: String::new(),
            targets: vec![],
            evidence: vec![],
        }
    }
}

/// 记忆冲突解决器 — 将反思元数据标准化为稳定的争议判定。
///
/// 对应 Python 版 `src/memory/memory_dispute_resolver.py`
pub struct MemoryDisputeResolver {
    config: MemoryDisputeConfig,
}

/// 反思结构（LLM 输出的冲突分析结果）
#[derive(Debug, Clone, Default)]
pub struct ReflectionPayload {
    pub has_conflict: bool,
    pub action: String,
    pub confidence: f64,
    pub conflict_type: String,
    pub summary: String,
    pub reason: String,
    pub targets: Vec<String>,
    pub evidence: Vec<String>,
}

impl MemoryDisputeResolver {
    pub fn new(config: MemoryDisputeConfig) -> Self {
        Self { config }
    }

    /// 从反思 payload 解析冲突判定
    pub fn resolve(&self, reflection: &ReflectionPayload) -> MemoryDisputeDecision {
        let has_conflict = reflection.has_conflict;
        let action = reflection.action.trim().to_lowercase();
        let confidence = normalize_confidence(reflection.confidence);

        if !has_conflict || action.is_empty() {
            return MemoryDisputeDecision {
                level: "ignore".to_string(),
                confidence,
                ..Default::default()
            };
        }

        let level = if confidence >= self.config.high_confidence_threshold {
            "high_confidence"
        } else if confidence >= self.config.normal_confidence_threshold {
            "normal"
        } else {
            "ignore"
        };

        let conflict_type = reflection.conflict_type.trim().to_string();
        let conflict_type = if conflict_type.is_empty() {
            "none".to_string()
        } else {
            conflict_type
        };

        MemoryDisputeDecision {
            level: level.to_string(),
            confidence,
            action,
            conflict_type,
            summary: reflection.summary.trim().to_string(),
            reason: reflection.reason.trim().to_string(),
            targets: reflection.targets.clone(),
            evidence: reflection.evidence.clone(),
        }
    }

    /// 从记忆元数据中提取反思信息并解析
    pub fn resolve_from_memory_metadata(
        &self,
        metadata: &serde_json::Value,
    ) -> MemoryDisputeDecision {
        let reflection = metadata
            .as_object()
            .and_then(|obj| obj.get("reflection"))
            .and_then(|v| v.as_object());

        match reflection {
            Some(obj) => {
                let payload = ReflectionPayload {
                    has_conflict: obj
                        .get("has_conflict")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    action: obj
                        .get("action")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    confidence: obj
                        .get("confidence")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0),
                    conflict_type: obj
                        .get("conflict_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("none")
                        .to_string(),
                    summary: obj
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    reason: obj
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    targets: obj
                        .get("targets")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    evidence: obj
                        .get("evidence")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                self.resolve(&payload)
            }
            None => MemoryDisputeDecision::default(),
        }
    }
}

/// 标准化置信度到 [0.0, 1.0] 范围
fn normalize_confidence(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> MemoryDisputeConfig {
        MemoryDisputeConfig {
            enabled: true,
            high_confidence_threshold: 0.75,
            normal_confidence_threshold: 0.45,
            signal_ttl_hours: 168.0,
        }
    }

    #[test]
    fn test_resolve_ignore_no_conflict() {
        let resolver = MemoryDisputeResolver::new(test_config());
        let reflection = ReflectionPayload {
            has_conflict: false,
            action: "keep".to_string(),
            confidence: 0.9,
            ..Default::default()
        };
        let decision = resolver.resolve(&reflection);
        assert_eq!(decision.level, "ignore");
    }

    #[test]
    fn test_resolve_ignore_empty_action() {
        let resolver = MemoryDisputeResolver::new(test_config());
        let reflection = ReflectionPayload {
            has_conflict: true,
            action: String::new(),
            confidence: 0.9,
            ..Default::default()
        };
        let decision = resolver.resolve(&reflection);
        assert_eq!(decision.level, "ignore");
    }

    #[test]
    fn test_resolve_high_confidence() {
        let resolver = MemoryDisputeResolver::new(test_config());
        let reflection = ReflectionPayload {
            has_conflict: true,
            action: "update".to_string(),
            confidence: 0.8,
            ..Default::default()
        };
        let decision = resolver.resolve(&reflection);
        assert_eq!(decision.level, "high_confidence");
        assert_eq!(decision.action, "update");
    }

    #[test]
    fn test_resolve_normal_confidence() {
        let resolver = MemoryDisputeResolver::new(test_config());
        let reflection = ReflectionPayload {
            has_conflict: true,
            action: "merge".to_string(),
            confidence: 0.5,
            ..Default::default()
        };
        let decision = resolver.resolve(&reflection);
        assert_eq!(decision.level, "normal");
    }

    #[test]
    fn test_resolve_low_confidence_ignored() {
        let resolver = MemoryDisputeResolver::new(test_config());
        let reflection = ReflectionPayload {
            has_conflict: true,
            action: "delete".to_string(),
            confidence: 0.3,
            ..Default::default()
        };
        let decision = resolver.resolve(&reflection);
        assert_eq!(decision.level, "ignore");
    }

    #[test]
    fn test_normalize_confidence_bounds() {
        assert_eq!(normalize_confidence(-0.5), 0.0);
        assert_eq!(normalize_confidence(1.5), 1.0);
        assert_eq!(normalize_confidence(0.7), 0.7);
    }

    #[test]
    fn test_resolve_from_metadata() {
        let resolver = MemoryDisputeResolver::new(test_config());
        let metadata = serde_json::json!({
            "reflection": {
                "has_conflict": true,
                "action": "update",
                "confidence": 0.8,
                "conflict_type": "contradiction",
                "summary": "旧信息与新信息冲突",
                "reason": "新来源更可靠",
                "targets": ["mem_1", "mem_2"],
                "evidence": ["用户本人确认"]
            }
        });

        let decision = resolver.resolve_from_memory_metadata(&metadata);
        assert_eq!(decision.level, "high_confidence");
        assert_eq!(decision.action, "update");
        assert_eq!(decision.conflict_type, "contradiction");
        assert_eq!(decision.targets, vec!["mem_1", "mem_2"]);
    }

    #[test]
    fn test_resolve_from_metadata_empty() {
        let resolver = MemoryDisputeResolver::new(test_config());
        let decision = resolver.resolve_from_memory_metadata(&serde_json::json!({}));
        assert_eq!(decision.level, "ignore");
    }

    #[test]
    fn test_default_decision() {
        let d = MemoryDisputeDecision::default();
        assert_eq!(d.level, "ignore");
        assert_eq!(d.confidence, 0.0);
    }
}
