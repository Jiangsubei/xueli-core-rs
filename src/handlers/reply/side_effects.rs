use std::sync::Arc;
use tokio::sync::Mutex;

use crate::handlers::reply::effect_tracker::{ReplyEffectScore, ReplyEffectTracker};

/// 回复副作用 — 发回复后的效果评估与后续处理
///
/// 对应 Python 版 `ReplySideEffects`
pub struct ReplySideEffects {
    effect_tracker: Arc<Mutex<ReplyEffectTracker>>,
}

impl ReplySideEffects {
    pub fn new(effect_tracker: Arc<Mutex<ReplyEffectTracker>>) -> Self {
        Self { effect_tracker }
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

    /// 评估回复效果：消费 pending 评估条目，调用 LLM 评分
    ///
    /// 返回 `None` 表示没有待处理的评估或评估失败（静默失败）
    pub async fn evaluate_reply_effect(
        &self,
        user_id: &str,
        group_id: &str,
        user_text: &str,
        _relationship_summary: &str,
    ) -> Option<ReplyEffectScore> {
        // 消费 pending 评估
        let pending = {
            let mut tracker = self.effect_tracker.lock().await;
            tracker.consume_pending(user_id, group_id)
        };

        let pending = pending?;

        // 调用 LLM 反馈分流（对应 feedback_triage signal）
        // 当前以简化方式实现：评分基于文本是否为空
        let score = self.compute_feedback_score(&pending, user_text).await;

        Some(score)
    }

    /// 计算反馈评分（简化版，后续可接入真实的 feedback_triage LLM）
    async fn compute_feedback_score(
        &self,
        pending: &crate::handlers::reply::effect_tracker::PendingEvaluation,
        user_text: &str,
    ) -> ReplyEffectScore {
        let label = classify_user_response(user_text, &pending.expected_effect);
        let actual_effect = match label.as_str() {
            "positive_engagement" => "continue".to_string(),
            "satisfied" => "satisfy".to_string(),
            "disengaged" => "cool_down".to_string(),
            "confused" => "clarify".to_string(),
            _ => "neutral".to_string(),
        };
        let expected_met =
            !pending.expected_effect.is_empty() && actual_effect == pending.expected_effect;

        let score = match label.as_str() {
            "positive_engagement" => 0.8,
            "satisfied" => 0.9,
            "disengaged" => 0.2,
            "confused" => 0.3,
            "neutral" => 0.5,
            _ => 0.5,
        };

        ReplyEffectScore {
            score,
            label,
            reply_intent: pending.reply_intent.clone(),
            feedback_label: String::new(),
            expected_effect: pending.expected_effect.clone(),
            actual_effect,
            expected_effect_met: Some(expected_met),
        }
    }

    /// 清理过期的 pending 条目
    pub async fn cleanup(&self) {
        let mut tracker = self.effect_tracker.lock().await;
        tracker.cleanup();
    }
}

/// 分类用户回复（简化版，基于关键词/长度启发式规则）
///
/// 注意：设计上应使用 LLM feedback_triage，这里作为 fallback 简化实现
fn classify_user_response(user_text: &str, _expected_effect: &str) -> String {
    let text = user_text.trim().to_lowercase();

    if text.is_empty() {
        return "neutral".to_string();
    }

    // 简短正面回应（含复合响应如 "好的谢谢"）
    let brief_positive = [
        "好",
        "嗯",
        "ok",
        "好的",
        "行",
        "可以",
        "谢谢",
        "感谢",
        "懂了",
        "好的谢谢",
        "好的感谢",
        "好的呢",
        "好呀",
        "好哦",
        "嗯嗯",
    ];
    for p in &brief_positive {
        if text == *p {
            return "satisfied".to_string();
        }
    }

    // 困惑响应（必须在追问之前检查）
    if text.contains("什么意思") || text.contains("不懂") || text.contains("没明白") {
        return "confused".to_string();
    }

    // 追问/继续互动
    if text.contains('?') || text.contains('？') || text.len() > 50 {
        return "positive_engagement".to_string();
    }

    // 简短冷淡回应
    if text.len() <= 3 {
        return "disengaged".to_string();
    }

    "neutral".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_record_and_evaluate() {
        let tracker = Arc::new(Mutex::new(ReplyEffectTracker::new(600.0)));
        let side_effects = ReplySideEffects::new(tracker.clone());

        side_effects
            .record_pending_evaluation("u1", "g1", "你好呀", "greet", "continue", "用户会继续聊")
            .await;

        // 用户回复了
        let score = side_effects
            .evaluate_reply_effect("u1", "g1", "你好！今天天气不错，去公园吗？", "")
            .await;
        assert!(score.is_some());
        let s = score.unwrap();
        // 长回复 -> positive_engagement
        assert_eq!(s.label, "positive_engagement");
        assert!(s.score > 0.5);
    }

    #[tokio::test]
    async fn test_evaluate_satisfied() {
        let tracker = Arc::new(Mutex::new(ReplyEffectTracker::new(600.0)));
        let side_effects = ReplySideEffects::new(tracker.clone());

        side_effects
            .record_pending_evaluation("u1", "g1", "答案", "answer", "satisfy", "")
            .await;

        let score = side_effects
            .evaluate_reply_effect("u1", "g1", "好的谢谢", "")
            .await;
        assert!(score.is_some());
        let s = score.unwrap();
        assert_eq!(s.label, "satisfied");
        assert_eq!(s.actual_effect, "satisfy");
        assert!(s.expected_effect_met.unwrap());
    }

    #[tokio::test]
    async fn test_evaluate_disengaged() {
        let tracker = Arc::new(Mutex::new(ReplyEffectTracker::new(600.0)));
        let side_effects = ReplySideEffects::new(tracker.clone());

        side_effects
            .record_pending_evaluation("u1", "g1", "消息", "chat", "", "")
            .await;

        let score = side_effects
            .evaluate_reply_effect("u1", "g1", "哦", "")
            .await;
        assert!(score.is_some());
        let s = score.unwrap();
        assert_eq!(s.label, "disengaged");
    }

    #[tokio::test]
    async fn test_no_pending_returns_none() {
        let tracker = Arc::new(Mutex::new(ReplyEffectTracker::new(600.0)));
        let side_effects = ReplySideEffects::new(tracker);

        let score = side_effects
            .evaluate_reply_effect("u1", "g1", "hello", "")
            .await;
        assert!(score.is_none());
    }

    #[test]
    fn test_classify_user_response() {
        assert_eq!(classify_user_response("好的", ""), "satisfied");
        assert_eq!(classify_user_response("谢谢", ""), "satisfied");
        assert_eq!(classify_user_response("好的谢谢", ""), "satisfied");
        assert_eq!(classify_user_response("哦", ""), "disengaged");
        assert_eq!(classify_user_response("这是什么意思？", ""), "confused");
        assert_eq!(
            classify_user_response("今天天气真好，我们去公园散步吧", ""),
            "neutral"
        );
        assert_eq!(
            classify_user_response("今天天气真好，一起去公园散步吗？", ""),
            "positive_engagement"
        );
        assert_eq!(classify_user_response("嗯嗯", ""), "satisfied");
    }
}
