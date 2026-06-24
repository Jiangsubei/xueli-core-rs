use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use crate::character::card_service::CharacterCardService;
use crate::handlers::reply::effect_tracker::{ReplyEffectScore, ReplyEffectTracker};
use crate::traits::FeedbackTriageProvider;

/// 回复副作用 — 发回复后的效果评估与后续处理
///
/// 对应 Python 版 `ReplySideEffectHandler`
pub struct ReplySideEffects {
    effect_tracker: Arc<Mutex<ReplyEffectTracker>>,
    /// 角色卡服务（可选，用于 apply_reply_effect）
    character_card_service: Option<Arc<CharacterCardService>>,
    /// 反馈分流信号提供者（可选，用于 LLM 评分）
    signal_orchestrator: Option<Arc<dyn FeedbackTriageProvider>>,
    /// 最近一次 feedback_triage 信号缓存
    last_feedback_triage: std::sync::Mutex<HashMap<String, Value>>,
}

impl ReplySideEffects {
    pub fn new(effect_tracker: Arc<Mutex<ReplyEffectTracker>>) -> Self {
        Self {
            effect_tracker,
            character_card_service: None,
            signal_orchestrator: None,
            last_feedback_triage: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// 设置角色卡服务
    pub fn with_character_card_service(mut self, svc: Arc<CharacterCardService>) -> Self {
        self.character_card_service = Some(svc);
        self
    }

    /// 设置反馈分流信号提供者（LLM 反馈评分源）
    pub fn with_signal_orchestrator(mut self, svc: Arc<dyn FeedbackTriageProvider>) -> Self {
        self.signal_orchestrator = Some(svc);
        self
    }

    /// 记录一条即将发出的回复，等待后续反馈评估
    pub async fn record_pending_evaluation(
        &self,
        user_id: &str,
        group_id: &str,
        reply_text: &str,
        reply_intent: &str,
        expected_effect: &str,
        predicted_response: &str,
    ) {
        let mut tracker = self.effect_tracker.lock().await;
        tracker.record_reply(
            user_id,
            group_id,
            reply_text,
            reply_intent,
            expected_effect,
            predicted_response,
        );
    }

    /// 评估回复效果：消费 pending 评估条目，调用 LLM 反馈评分
    ///
    /// 返回 `None` 表示没有待处理的评估、未配置反馈评分源、或 LLM 调用失败 /
    /// 返回无效结果。此时不得向提示词注入伪造的语义信号。
    pub async fn evaluate_reply_effect(
        &self,
        user_id: &str,
        group_id: &str,
        message_id: &str,
        user_text: &str,
        relationship_summary: &str,
    ) -> Option<ReplyEffectScore> {
        // 消费 pending 评估
        let pending = {
            let mut tracker = self.effect_tracker.lock().await;
            tracker.consume_pending(user_id, group_id)
        };

        let pending = pending?;
        let orchestrator = self.signal_orchestrator.as_ref()?;

        let scope_key = format!("{}:{}", group_id, user_id);
        let signal = orchestrator
            .get_or_compute_feedback_triage_signal(
                &scope_key,
                message_id,
                &pending.reply_text,
                user_text,
                &pending.expected_effect,
                &pending.predicted_response,
                relationship_summary,
            )
            .await?;

        let label = signal
            .get("reply_effect_label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();

        // Python 仅接受这三个语义反馈标签，其它视为无效结果
        if !matches!(label.as_str(), "positive" | "negative" | "repair") {
            return None;
        }

        let score = signal
            .get("reply_effect_score")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let actual_effect = signal
            .get("actual_effect")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let expected_effect_met = signal.get("expected_effect_met").and_then(|v| v.as_bool());

        // 缓存完整信号，供 apply_reply_effect 提取 style_label / emotion_label
        let triage_key = format!("{}:{}", group_id, user_id);
        if let Ok(mut cache) = self.last_feedback_triage.lock() {
            cache.insert(triage_key, signal);
        }

        Some(ReplyEffectScore {
            score,
            label: label.clone(),
            reply_intent: pending.reply_intent.clone(),
            feedback_label: label,
            expected_effect: pending.expected_effect.clone(),
            actual_effect,
            expected_effect_met,
        })
    }

    /// 清理过期的 pending 条目
    pub async fn cleanup(&self) {
        let mut tracker = self.effect_tracker.lock().await;
        tracker.cleanup();
    }

    /// 应用回复效果到角色卡
    ///
    /// 对应 Python 版 `apply_reply_effect()`
    pub async fn apply_reply_effect(
        &self,
        user_id: &str,
        group_id: &str,
        score: &ReplyEffectScore,
    ) {
        let reply_intent = score.reply_intent.trim().to_string();
        let feedback_label = if score.feedback_label.is_empty() {
            score.label.clone()
        } else {
            score.feedback_label.clone()
        };

        // 消费缓存的 feedback_triage 信号，避免重复应用或内存泄漏
        let triage_key = format!("{}:{}", group_id, user_id);
        let signal = self
            .last_feedback_triage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&triage_key)
            .unwrap_or_default();

        let ccs = match &self.character_card_service {
            Some(s) => s,
            None => return,
        };
        let style_label = signal
            .get("style_feedback_label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let emotion_label = signal
            .get("emotion_label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let prediction_met = signal.get("prediction_met").and_then(|v| v.as_bool());
        let actual_effect = signal
            .get("actual_effect")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        // 尝试调用 apply_reply_effect_bundle
        let applied = ccs.apply_reply_effect_bundle(
            user_id,
            score.score,
            &feedback_label,
            &reply_intent,
            &style_label,
            &emotion_label,
            &score.expected_effect,
            &actual_effect,
            prediction_met,
        );

        if applied.is_err() {
            // 简化路径：记录交互信号
            match score.label.as_str() {
                "negative" => {
                    let _ = ccs.record_interaction_signal(user_id, "reply_negative");
                }
                "positive" => {
                    let _ = ccs.record_interaction_signal(user_id, "reply_positive");
                }
                "repair" => {
                    let _ = ccs.record_interaction_signal(user_id, "reply_repair");
                }
                _ => {}
            }

            if !reply_intent.is_empty() && !feedback_label.is_empty() {
                let _ = ccs.record_interaction_signal(
                    user_id,
                    &format!("reply_effect:{}:{}", reply_intent, feedback_label),
                );
            }

            if !feedback_label.is_empty() {
                let _ = ccs.record_reply_feedback(user_id, &feedback_label, score.score);
            }

            if !style_label.is_empty() {
                let _ = ccs.record_explicit_feedback_category(user_id, &style_label, "");
            }

            if !emotion_label.is_empty() {
                let _ = ccs.record_emotion(user_id, &emotion_label);
            }

            ccs.refresh_snapshot(user_id);
        }
    }

    /// 带超时执行异步任务
    ///
    /// 对应 Python 版 `run_with_timeout()`
    pub async fn run_with_timeout<F, T>(
        future: F,
        timeout_seconds: f64,
        fallback_value: T,
        label: &str,
    ) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let timeout = std::time::Duration::from_secs_f64(timeout_seconds.max(0.05));
        match tokio::time::timeout(timeout, future).await {
            Ok(result) => result,
            Err(_) => {
                tracing::debug!("[副作用] {}超时，走降级", label);
                fallback_value
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    fn sample_signal(label: &str, score: f64, actual_effect: &str) -> Value {
        serde_json::json!({
            "reply_effect_label": label,
            "reply_effect_score": score,
            "actual_effect": actual_effect,
            "expected_effect_met": true,
            "style_feedback_label": "too_formal",
            "emotion_label": "happy",
            "prediction_met": true,
            "confidence": 0.9,
        })
    }

    struct MockProvider {
        response: Option<Value>,
    }

    #[async_trait]
    impl FeedbackTriageProvider for MockProvider {
        async fn get_or_compute_feedback_triage_signal(
            &self,
            _scope_key: &str,
            _message_id: &str,
            _reply_text: &str,
            _user_text: &str,
            _expected_effect: &str,
            _predicted_response: &str,
            _relationship_summary: &str,
        ) -> Option<Value> {
            self.response.clone()
        }
    }

    fn side_effects_with_provider(
        provider: Option<Arc<dyn FeedbackTriageProvider>>,
    ) -> ReplySideEffects {
        let tracker = Arc::new(Mutex::new(ReplyEffectTracker::new(600.0)));
        let mut se = ReplySideEffects::new(tracker);
        if let Some(p) = provider {
            se = se.with_signal_orchestrator(p);
        }
        se
    }

    #[tokio::test]
    async fn test_record_and_evaluate_positive() {
        let se = side_effects_with_provider(Some(Arc::new(MockProvider {
            response: Some(sample_signal("positive", 0.8, "continue")),
        })));

        se.record_pending_evaluation("u1", "g1", "你好呀", "greet", "continue", "用户会继续聊")
            .await;

        let score = se
            .evaluate_reply_effect("u1", "g1", "m1", "你好！今天天气不错，去公园吗？", "")
            .await;
        assert!(score.is_some());
        let s = score.unwrap();
        assert_eq!(s.label, "positive");
        assert_eq!(s.feedback_label, "positive");
        assert_eq!(s.actual_effect, "continue");
        assert!(s.score > 0.5);
        assert_eq!(s.expected_effect_met, Some(true));
    }

    #[tokio::test]
    async fn test_evaluate_negative() {
        let se = side_effects_with_provider(Some(Arc::new(MockProvider {
            response: Some(sample_signal("negative", -0.6, "clarify")),
        })));

        se.record_pending_evaluation("u1", "g1", "答案", "answer", "satisfy", "")
            .await;

        let score = se
            .evaluate_reply_effect("u1", "g1", "m2", "这不是我要的答案", "")
            .await;
        assert!(score.is_some());
        let s = score.unwrap();
        assert_eq!(s.label, "negative");
        assert!(s.score < 0.0);
        assert_eq!(s.actual_effect, "clarify");
    }

    #[tokio::test]
    async fn test_evaluate_invalid_label_returns_none() {
        // LLM 返回了不允许的标签（如 "satisfied"），视为无效结果，不得伪造信号
        let se = side_effects_with_provider(Some(Arc::new(MockProvider {
            response: Some(sample_signal("satisfied", 0.9, "satisfy")),
        })));

        se.record_pending_evaluation("u1", "g1", "消息", "chat", "", "")
            .await;

        let score = se
            .evaluate_reply_effect("u1", "g1", "m3", "好的谢谢", "")
            .await;
        assert!(score.is_none());
    }

    #[tokio::test]
    async fn test_evaluate_orchestrator_failure_returns_none() {
        let se = side_effects_with_provider(Some(Arc::new(MockProvider { response: None })));

        se.record_pending_evaluation("u1", "g1", "消息", "chat", "", "")
            .await;

        let score = se.evaluate_reply_effect("u1", "g1", "m4", "哦", "").await;
        assert!(score.is_none());
    }

    #[tokio::test]
    async fn test_no_orchestrator_returns_none() {
        let se = side_effects_with_provider(None);

        se.record_pending_evaluation("u1", "g1", "消息", "chat", "", "")
            .await;

        let score = se
            .evaluate_reply_effect("u1", "g1", "m5", "用户回复", "")
            .await;
        assert!(score.is_none());
    }

    #[tokio::test]
    async fn test_no_pending_returns_none() {
        let se = side_effects_with_provider(Some(Arc::new(MockProvider {
            response: Some(sample_signal("positive", 0.8, "continue")),
        })));

        let score = se
            .evaluate_reply_effect("u1", "g1", "m6", "hello", "")
            .await;
        assert!(score.is_none());
    }

    #[tokio::test]
    async fn test_apply_reply_effect_caches_signal() {
        let se = side_effects_with_provider(Some(Arc::new(MockProvider {
            response: Some(sample_signal("positive", 0.8, "continue")),
        })));

        se.record_pending_evaluation("u1", "g1", "你好", "greet", "continue", "")
            .await;

        let score = se
            .evaluate_reply_effect("u1", "g1", "m7", "用户回复", "")
            .await
            .expect("应返回评分");

        // 验证缓存被写入：apply_reply_effect 需要能读到 style_label / emotion_label
        let cached = se
            .last_feedback_triage
            .lock()
            .unwrap()
            .get("g1:u1")
            .cloned();
        assert!(cached.is_some());
        let cached = cached.unwrap();
        assert_eq!(
            cached.get("style_feedback_label").and_then(|v| v.as_str()),
            Some("too_formal")
        );
        assert_eq!(
            cached.get("emotion_label").and_then(|v| v.as_str()),
            Some("happy")
        );

        // apply_reply_effect 在缺少 character_card_service 时应直接返回
        se.apply_reply_effect("u1", "g1", &score).await;

        // 调用后缓存应被消费（移除）
        assert!(!se
            .last_feedback_triage
            .lock()
            .unwrap()
            .contains_key("g1:u1"));
    }
}
