use async_trait::async_trait;

use crate::prelude::XueliResult;
use crate::core::platform_types::InboundEvent;

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
}

/// TimingGate 策略 trait
#[async_trait]
pub trait TimingGateStrategy: Send + Sync {
    /// 判断是否应该在此时回复
    async fn should_reply(&self, context: &TimingContext) -> XueliResult<TimingDecision>;
}
