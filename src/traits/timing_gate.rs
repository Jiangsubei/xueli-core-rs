use async_trait::async_trait;

use crate::core::platform_types::InboundEvent;
use crate::prelude::XueliResult;

/// 时机判断决策
#[derive(Debug, Clone, PartialEq)]
pub enum TimingDecision {
    /// 立即回复
    Reply,
    /// 等待，继续观察
    Wait(f64),
    /// 忽略此消息
    Ignore,
}

/// 时机判断上下文
#[derive(Debug, Clone)]
pub struct TimingContext {
    pub event: InboundEvent,
    pub is_mentioned: bool,
    pub conversation_active: bool,
    pub time_since_last_reply_secs: f64,
    pub message_count_in_window: u32,
    /// 预渲染的近期历史文本（由调用方通过 unified_history_renderer 生成）
    pub recent_history_text: String,
    /// wait 决策后新到达的消息文本列表（用于重新评估）
    pub new_messages_since_wait: Vec<String>,
}

impl Default for TimingContext {
    fn default() -> Self {
        Self {
            event: InboundEvent::default(),
            is_mentioned: false,
            conversation_active: false,
            time_since_last_reply_secs: 0.0,
            message_count_in_window: 0,
            recent_history_text: String::new(),
            new_messages_since_wait: Vec::new(),
        }
    }
}

/// TimingGate 策略 trait
#[async_trait]
pub trait TimingGateStrategy: Send + Sync {
    /// 判断是否应该在此时回复
    async fn should_reply(&self, context: &TimingContext) -> XueliResult<TimingDecision>;
}
