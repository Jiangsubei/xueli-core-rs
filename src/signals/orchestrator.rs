use crate::core::platform_types::InboundEvent;
use crate::core::scope::ChatScope;
use crate::signals::engagement::build_message_observations;
use crate::signals::temporal::TemporalContext;

use crate::prelude::XueliResult;

/// 语义信号编排器 — 从消息中提取多层语义信号
///
/// 对应 Python 版 `xueli/src/handlers/signals/orchestrator.py`
/// 当前实现为纯结构化信号提取，不含 LLM 调用（L1/L2 缓存 + LLM 调用后续接入）
pub struct SignalOrchestrator;

/// 综合语义信号
#[derive(Debug, Clone)]
pub struct SemanticSignals {
    pub temporal: Option<TemporalContext>,
    pub metacognition: MetacognitionSignal,
    pub engagement: EngagementSignal,
    pub observations: ObservationsSignal,
}

/// 元认知信号
#[derive(Debug, Clone, Default)]
pub struct MetacognitionSignal {
    pub caution_level: String,
    pub caution_reasons: Vec<String>,
}

/// 互动参与信号
#[derive(Debug, Clone, Default)]
pub struct EngagementSignal {
    pub message_length: usize,
    pub question_count: usize,
    pub emoji_count: usize,
}

/// 消息观察信号（结构化可观测事实）
#[derive(Debug, Clone, Default)]
pub struct ObservationsSignal {
    pub message_length_bucket: String,
    pub is_short_message: bool,
    pub is_light_response_candidate: bool,
    pub is_continuation_candidate: bool,
    pub assistant_replied_recently: bool,
    pub follows_assistant_recently: bool,
    pub same_user_continuation: bool,
    pub recent_history_count: usize,
}

impl SignalOrchestrator {
    pub fn new() -> Self {
        Self
    }

    /// 从事件中提取结构化信号（不含 LLM 调用）
    pub fn extract(&self, event: &InboundEvent) -> SemanticSignals {
        let (text, sender_id) = match &event.message {
            Some(msg) => (msg.text.as_str(), msg.sender_id.as_str()),
            None => ("", ""),
        };

        // 参与度信号
        let engagement = EngagementSignal {
            message_length: text.chars().count(),
            question_count: text.matches('?').count() + text.matches('？').count(),
            emoji_count: count_emoji(text),
        };

        // 消息观察信号
        let observations = build_message_observations(
            text, sender_id, "", // 上一条消息角色：由调用方注入
            "", // 上一条用户消息发送者：由调用方注入
            "unknown", 0,
        );

        SemanticSignals {
            temporal: None, // 由调用方通过 build_temporal_context 设置
            metacognition: MetacognitionSignal::default(),
            engagement,
            observations: ObservationsSignal {
                message_length_bucket: observations.message_length_bucket,
                is_short_message: observations.is_short_message,
                is_light_response_candidate: observations.is_light_response_candidate,
                is_continuation_candidate: observations.is_continuation_candidate,
                assistant_replied_recently: observations.assistant_replied_recently,
                follows_assistant_recently: observations.follows_assistant_recently,
                same_user_continuation: observations.same_user_continuation,
                recent_history_count: observations.recent_history_count,
            },
        }
    }
}

impl Default for SignalOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

/// 简单统计文本中的 emoji 数量
fn count_emoji(text: &str) -> usize {
    text.chars()
        .filter(|c| {
            let cp = *c as u32;
            // 常见 emoji 范围
            (0x1F600..=0x1F64F).contains(&cp)    // 表情符号
                || (0x1F300..=0x1F5FF).contains(&cp)  // 杂项符号和象形文字
                || (0x1F680..=0x1F6FF).contains(&cp)  // 交通和地图符号
                || (0x2600..=0x26FF).contains(&cp)    // 杂项符号
                || (0x2700..=0x27BF).contains(&cp)    // 装饰符号
                || (0x1F900..=0x1F9FF).contains(&cp)  // 补充符号和象形文字
                || (0x1FA00..=0x1FA6F).contains(&cp)  // 棋牌符号
                || (0x1FA70..=0x1FAFF).contains(&cp)  // 扩展符号和象形文字
                || (0xFE00..=0xFE0F).contains(&cp)    // 变体选择器
                || cp == 0x200D // 零宽连接符 (ZWJ)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::platform_types::EventType;
    use crate::core::types::UserMessage;
    use chrono::Utc;

    fn make_event(text: &str) -> InboundEvent {
        InboundEvent {
            id: "msg1".to_string(),
            platform: "test".to_string(),
            event_type: EventType::Message,
            message: Some(UserMessage {
                id: "msg1".to_string(),
                sender_id: "user1".to_string(),
                sender_name: "测试用户".to_string(),
                text: text.to_string(),
                timestamp: Utc::now(),
                scope: ChatScope::Private,
                is_mention: false,
            }),
            raw_payload: None,
            received_at: Utc::now(),
            session: None,
        }
    }

    #[test]
    fn test_extract_signals() {
        let orchestrator = SignalOrchestrator::new();
        let event = make_event("你好吗？今天天气很好呢！");
        let signals = orchestrator.extract(&event);
        assert_eq!(signals.engagement.question_count, 1);
        assert!(signals.engagement.message_length > 0);
    }

    #[test]
    fn test_extract_signals_empty() {
        let orchestrator = SignalOrchestrator::new();
        let event = make_event("");
        let signals = orchestrator.extract(&event);
        assert_eq!(signals.engagement.message_length, 0);
        assert_eq!(signals.engagement.question_count, 0);
    }

    #[test]
    fn test_extract_observations_short_message() {
        let orchestrator = SignalOrchestrator::new();
        let event = make_event("嗯");
        let signals = orchestrator.extract(&event);
        assert!(signals.observations.is_short_message);
    }
}
