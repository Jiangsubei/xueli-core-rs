use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::config::MemoryConfig;

/// 后台协调器 — 离线记忆维护（消化、反思、清理）
pub struct BackgroundCoordinator {
    config: Arc<MemoryConfig>,
    /// 是否正在运行
    running: Arc<RwLock<bool>>,
}

impl BackgroundCoordinator {
    pub fn new(config: Arc<MemoryConfig>) -> Self {
        Self {
            config,
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// 启动后台循环
    pub async fn start(&self) {
        let mut running = self.running.write().await;
        *running = true;
        drop(running);

        // TODO: 启动后台消化循环
    }

    /// 停止后台循环
    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
    }
}