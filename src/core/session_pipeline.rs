use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use dashmap::DashMap;

use crate::core::platform_types::InboundEvent;

/// 每会话串行 worker — 保证同一会话消息串行处理
pub struct SessionPipeline {
    /// 会话 → 消息发送器 映射
    sessions: Arc<DashMap<String, mpsc::UnboundedSender<InboundEvent>>>,
    /// 会话锁（确保串行）
    locks: Arc<DashMap<String, Arc<Mutex<()>>>>,
}

impl SessionPipeline {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            locks: Arc::new(DashMap::new()),
        }
    }

    /// 向指定会话投递事件
    pub fn send(&self, session_id: &str, event: InboundEvent) -> Result<(), String> {
        if let Some(tx) = self.sessions.get(session_id) {
            tx.send(event).map_err(|e| format!("会话消息投递失败: {}", e))?;
        }
        Ok(())
    }

    /// 注册新会话
    pub fn register_session(&self, session_id: String, tx: mpsc::UnboundedSender<InboundEvent>) {
        self.sessions.insert(session_id.clone(), tx);
        self.locks
            .insert(session_id, Arc::new(Mutex::new(())));
    }

    /// 获取会话锁
    pub fn get_lock(&self, session_id: &str) -> Option<Arc<Mutex<()>>> {
        self.locks.get(session_id).map(|l| l.clone())
    }
}

impl Default for SessionPipeline {
    fn default() -> Self {
        Self::new()
    }
}