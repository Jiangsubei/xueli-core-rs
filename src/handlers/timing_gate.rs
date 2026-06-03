use std::sync::Arc;

use crate::core::config::TimingGateConfig;
use crate::core::platform_types::InboundEvent;
use crate::traits::timing_gate::{TimingContext, TimingDecision, TimingGateStrategy};

/// 默认 TimingGate 实现
pub struct DefaultTimingGate {
    config: Arc<TimingGateConfig>,
}

impl DefaultTimingGate {
    pub fn new(config: Arc<TimingGateConfig>) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl TimingGateStrategy for DefaultTimingGate {
    async fn should_reply(&self, context: &TimingContext) -> Result<TimingDecision, String> {
        // 被 @ 时大概率回复
        if context.is_mentioned {
            if rand::random::<f64>() < self.config.mention_reply_probability {
                return Ok(TimingDecision::Reply);
            }
        }

        // 主动回复概率判断
        if context.conversation_active
            && context.time_since_last_reply_secs > 5.0
            && rand::random::<f64>() < self.config.default_proactive_probability
        {
            return Ok(TimingDecision::Reply);
        }

        // 否则等待一段时间
        Ok(TimingDecision::Wait(
            (rand::random::<f64>() * 30.0 + 5.0).min(60.0),
        ))
    }
}