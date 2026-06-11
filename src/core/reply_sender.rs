use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::Rng;
use rand::SeedableRng;

use crate::core::metrics::RuntimeMetrics;
use crate::core::platform_types::ReplyAction;
use crate::core::scope::ChatScope;
use crate::prelude::XueliResult;
use crate::traits::platform_adapter::PlatformAdapter;

/// 单段回复的计划（文本 + 发送前等待时间）
#[derive(Debug, Clone)]
pub struct ReplyPartPlan {
    pub text: String,
    /// 发送此段前需等待的秒数
    pub delay_before_seconds: f64,
}

/// 分段编排器 — 归一化分段、去重、计算每段延迟。
pub struct ReplySendOrchestrator {
    rng: rand::rngs::StdRng,
}

impl ReplySendOrchestrator {
    pub fn new() -> Self {
        Self {
            rng: rand::rngs::StdRng::from_entropy(),
        }
    }

    /// 归一化段列表：去掉空文本和连续重复文本，限制最大段数
    pub fn normalize_segments(
        &self,
        segments: &[String],
        fallback_text: &str,
        max_segments: usize,
    ) -> Vec<String> {
        let cleaned = self.clean_texts(segments);
        if cleaned.is_empty() {
            let fb = fallback_text.trim().to_string();
            return if fb.is_empty() { vec![] } else { vec![fb] };
        }
        let limit = std::cmp::max(1, max_segments);
        cleaned.into_iter().take(limit).collect()
    }

    /// 构建分段发送计划（含延迟）
    pub fn build_part_plan(
        &mut self,
        segments: &[String],
        fallback_text: &str,
        max_segments: usize,
        first_delay_min_ms: u64,
        first_delay_max_ms: u64,
        followup_delay_min_secs: f64,
        followup_delay_max_secs: f64,
    ) -> Vec<ReplyPartPlan> {
        let normalized = self.normalize_segments(segments, fallback_text, max_segments);
        let mut plans = Vec::new();

        for (index, text) in normalized.into_iter().enumerate() {
            let delay = if index == 0 {
                if max_segments <= 1 {
                    0.0
                } else {
                    self.uniform_seconds(
                        first_delay_min_ms as f64 / 1000.0,
                        first_delay_max_ms as f64 / 1000.0,
                    )
                }
            } else {
                self.uniform_seconds(followup_delay_min_secs, followup_delay_max_secs)
            };
            plans.push(ReplyPartPlan {
                text,
                delay_before_seconds: delay,
            });
        }
        plans
    }

    /// 去重去空
    fn clean_texts(&self, items: &[String]) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        let mut previous = String::new();
        for raw in items {
            let text = raw.trim().to_string();
            if text.is_empty() {
                continue;
            }
            if text == previous {
                continue;
            }
            previous = text.clone();
            result.push(text);
        }
        result
    }

    /// 在 [min, max] 范围内均匀随机
    fn uniform_seconds(&mut self, min: f64, max: f64) -> f64 {
        let lower = min.max(0.0);
        let upper = max.max(0.0);
        if upper <= lower {
            return lower;
        }
        self.rng.gen_range(lower..upper)
    }
}

impl Default for ReplySendOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

/// 回复发送器 — 通过 PlatformAdapter 发送回复，支持分段和延迟。
pub struct ReplySender<P: PlatformAdapter> {
    adapter: Arc<P>,
    orchestrator: ReplySendOrchestrator,
    metrics: Option<Arc<RuntimeMetrics>>,
}

impl<P: PlatformAdapter> ReplySender<P> {
    pub fn new(adapter: Arc<P>) -> Self {
        Self {
            adapter,
            orchestrator: ReplySendOrchestrator::new(),
            metrics: None,
        }
    }

    /// 设置运行时指标
    pub fn with_metrics(mut self, metrics: Arc<RuntimeMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 发送回复
    pub async fn send(&self, action: &ReplyAction) -> XueliResult<()> {
        self.adapter.send_action(action).await
    }

    /// 发送多条回复
    pub async fn send_batch(&self, actions: &[ReplyAction]) -> XueliResult<()> {
        for action in actions {
            self.adapter.send_action(action).await?;
        }
        Ok(())
    }

    /// 按分段计划发送（含延迟），每段之间等待相应秒数
    pub async fn send_segmented(
        &self,
        plans: &[ReplyPartPlan],
        scope: ChatScope,
        _reply_to: Option<&str>,
    ) -> XueliResult<()> {
        for plan in plans {
            if plan.delay_before_seconds > 0.0 {
                tokio::time::sleep(std::time::Duration::from_secs_f64(
                    plan.delay_before_seconds,
                ))
                .await;
            }
            let action = ReplyAction {
                scope: scope.clone(),
                text: plan.text.clone(),
                reply_to: None,
                image_url: None,
                emoji_id: None,
            };
            self.adapter.send_action(&action).await?;
        }
        if let Some(ref metrics) = self.metrics {
            metrics.record_reply_parts_sent(plans.len() as u64);
        }
        Ok(())
    }

    /// 将回复文本按空行切段后分段发送
    pub async fn send_with_auto_segments(
        &mut self,
        text: &str,
        scope: ChatScope,
        max_segments: usize,
    ) -> XueliResult<()> {
        // 按空行切分
        let raw_segments: Vec<String> = text
            .split("\n\n")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let plans =
            self.orchestrator
                .build_part_plan(&raw_segments, text, max_segments, 0, 600, 3.0, 10.0);

        self.send_segmented(&plans, scope, None).await
    }

    /// 检查窗口是否已过期
    pub fn is_window_expired(expires_at: f64) -> bool {
        if expires_at <= 0.0 {
            return false;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        now > expires_at
    }

    /// 构建引用回复段（通用格式，下游适配器可覆盖）
    pub fn build_quote_segment(message_id: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "reply",
            "data": { "id": message_id }
        })
    }

    /// 构建 @ 提及段（通用格式，下游适配器可覆盖）
    pub fn build_mention_payload(user_id: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "mention",
            "data": { "user_id": user_id }
        })
    }

    /// 确保出站消息中没有普通图片段
    pub fn ensure_no_outbound_images(segments: &[serde_json::Value]) -> XueliResult<()> {
        for segment in segments {
            let seg_type = segment
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if seg_type == "image" {
                return Err("当前运行模式禁止主动发送普通 image 段".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// 测试用空实现 PlatformAdapter
    struct NoopPlatformAdapter;

    #[async_trait]
    impl PlatformAdapter for NoopPlatformAdapter {
        async fn send_action(&self, _action: &ReplyAction) -> crate::prelude::XueliResult<()> {
            Ok(())
        }
        fn strip_mentions(&self, text: &str) -> String { text.to_string() }
        fn extract_mentions(&self, _event: &crate::core::platform_types::InboundEvent) -> Vec<String> { vec![] }
        fn resolve_mention_placeholders(&self, text: &str, _mentions: &[String]) -> String { text.to_string() }
        fn platform_name(&self) -> &str { "test" }
        fn parse_event(&self, _raw: &str) -> crate::prelude::XueliResult<crate::core::platform_types::InboundEvent> {
            unimplemented!()
        }
    }

    #[test]
    fn test_normalize_segments_dedup() {
        let orch = ReplySendOrchestrator::new();
        let segments = vec![
            "a".to_string(),
            "a".to_string(),
            "b".to_string(),
            "".to_string(),
        ];
        let result = orch.normalize_segments(&segments, "fallback", 10);
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn test_normalize_segments_empty_fallback() {
        let orch = ReplySendOrchestrator::new();
        let result = orch.normalize_segments(&[], "fallback text", 10);
        assert_eq!(result, vec!["fallback text"]);
    }

    #[test]
    fn test_normalize_segments_max() {
        let orch = ReplySendOrchestrator::new();
        let segments: Vec<String> = (0..10).map(|i| format!("s{}", i)).collect();
        let result = orch.normalize_segments(&segments, "fb", 3);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_build_part_plan() {
        let mut orch = ReplySendOrchestrator::new();
        let segments: Vec<String> = vec!["段一".into(), "段二".into(), "段三".into()];
        let plans = orch.build_part_plan(&segments, "fallback", 5, 0, 600, 3.0, 10.0);
        assert_eq!(plans.len(), 3);
        // 第一段延迟应在 0~0.6 秒，后续段在 3~10 秒
        assert!(plans[0].delay_before_seconds <= 0.6);
        assert!(plans[1].delay_before_seconds >= 3.0);
    }

    #[test]
    fn test_build_quote_segment() {
        let seg = ReplySender::<NoopPlatformAdapter>::build_quote_segment("12345");
        assert_eq!(seg["type"], "reply");
        assert_eq!(seg["data"]["id"], "12345");
    }

    #[test]
    fn test_build_mention_payload() {
        let seg = ReplySender::<NoopPlatformAdapter>::build_mention_payload("user_001");
        assert_eq!(seg["type"], "mention");
        assert_eq!(seg["data"]["user_id"], "user_001");
    }

    #[test]
    fn test_ensure_no_outbound_images_ok() {
        let segments = vec![
            serde_json::json!({"type": "text", "data": {"text": "hello"}}),
            serde_json::json!({"type": "mention", "data": {"user_id": "123"}}),
        ];
        assert!(ReplySender::<NoopPlatformAdapter>::ensure_no_outbound_images(&segments).is_ok());
    }

    #[test]
    fn test_ensure_no_outbound_images_reject() {
        let segments = vec![
            serde_json::json!({"type": "image", "data": {"url": "http://example.com/img.png"}}),
        ];
        assert!(ReplySender::<NoopPlatformAdapter>::ensure_no_outbound_images(&segments).is_err());
    }

    #[test]
    fn test_is_window_expired_not_set() {
        assert!(!ReplySender::<NoopPlatformAdapter>::is_window_expired(0.0));
    }

    #[test]
    fn test_is_window_expired_past() {
        // 使用一个过去的时间戳
        let past = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            - 100.0;
        assert!(ReplySender::<NoopPlatformAdapter>::is_window_expired(past));
    }

    #[test]
    fn test_is_window_expired_future() {
        // 使用一个未来的时间戳
        let future = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            + 3600.0;
        assert!(!ReplySender::<NoopPlatformAdapter>::is_window_expired(future));
    }
}
