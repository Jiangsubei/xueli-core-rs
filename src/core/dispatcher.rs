use std::sync::Arc;
use tokio::sync::mpsc;

use crate::core::errors::XueliResult;
use crate::core::platform_types::InboundEvent;
use crate::core::runtime::BotRuntime;

/// 事件分发器 — 将入站事件路由到对应处理器
pub struct EventDispatcher {
    runtime: Arc<BotRuntime>,
    event_tx: mpsc::UnboundedSender<InboundEvent>,
}

impl EventDispatcher {
    pub fn new(runtime: Arc<BotRuntime>, event_tx: mpsc::UnboundedSender<InboundEvent>) -> Self {
        Self { runtime, event_tx }
    }

    /// 分发入站事件
    pub fn dispatch(&self, event: InboundEvent) -> XueliResult<()> {
        self.event_tx
            .send(event)
            .map_err(|e| crate::core::errors::XueliError::Pipeline(format!("事件分发失败: {}", e)))
    }
}