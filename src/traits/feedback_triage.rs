use async_trait::async_trait;
use serde_json::Value;

/// 反馈分流信号提供者抽象
///
/// 用于 `ReplySideEffects` 注入 `SignalOrchestrator` 等 LLM 反馈评分源，
/// 避免与具体实现类型强耦合。
#[async_trait]
pub trait FeedbackTriageProvider: Send + Sync {
    /// 获取或计算本次回复的反馈分流信号
    ///
    /// 返回 `None` 表示 LLM 调用失败或返回无效结果，调用方不得伪造语义信号。
    #[allow(clippy::too_many_arguments)]
    async fn get_or_compute_feedback_triage_signal(
        &self,
        scope_key: &str,
        message_id: &str,
        reply_text: &str,
        user_text: &str,
        expected_effect: &str,
        predicted_response: &str,
        relationship_summary: &str,
    ) -> Option<Value>;
}
