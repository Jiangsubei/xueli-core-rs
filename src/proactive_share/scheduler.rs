use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::config::ProactiveShareConfig;

/// 主动分享调度器
pub struct ProactiveShareScheduler {
    config: Arc<ProactiveShareConfig>,
    running: Arc<RwLock<bool>>,
}

impl ProactiveShareScheduler {
    pub fn new(config: Arc<ProactiveShareConfig>) -> Self {
        Self {
            config,
            running: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn start(&self) {
        // TODO: 启动定时分享调度
    }

    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
    }
}