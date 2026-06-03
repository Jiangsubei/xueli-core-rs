use std::sync::Arc;

use crate::core::platform_types::ReplyAction;
use crate::traits::platform_adapter::PlatformAdapter;

/// 回复发送器 — 将通过 PlatformAdapter 发送回复
pub struct ReplySender<P: PlatformAdapter> {
    adapter: Arc<P>,
}

impl<P: PlatformAdapter> ReplySender<P> {
    pub fn new(adapter: Arc<P>) -> Self {
        Self { adapter }
    }

    /// 发送回复
    pub async fn send(&self, action: &ReplyAction) -> Result<(), String> {
        self.adapter.send_action(action).await
    }

    /// 发送多条回复
    pub async fn send_batch(&self, actions: &[ReplyAction]) -> Result<(), String> {
        for action in actions {
            self.adapter.send_action(action).await?;
        }
        Ok(())
    }
}