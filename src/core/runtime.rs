use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::config::XueliConfig;
use crate::core::metrics::RuntimeMetrics;

/// Bot 运行时 — 整个系统的生命周期管理中心
///
/// 对应 Python 版 `xueli/src/core/runtime.py`
pub struct BotRuntime {
    /// 配置
    pub config: Arc<XueliConfig>,
    /// 运行时指标
    pub metrics: Arc<RwLock<RuntimeMetrics>>,
    /// 运行状态
    state: Arc<RwLock<RuntimeState>>,
}

/// 运行时状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeState {
    Created,
    Initializing,
    Running,
    Stopping,
    Stopped,
}

impl BotRuntime {
    pub fn new(config: XueliConfig) -> Self {
        Self {
            config: Arc::new(config),
            metrics: Arc::new(RwLock::new(RuntimeMetrics::default())),
            state: Arc::new(RwLock::new(RuntimeState::Created)),
        }
    }

    /// 初始化运行时
    pub async fn init(&self) -> Result<(), String> {
        let mut state = self.state.write().await;
        if *state != RuntimeState::Created {
            return Err(format!("不能从状态 {:?} 初始化", *state));
        }
        *state = RuntimeState::Initializing;
        // TODO: 初始化各子系统
        *state = RuntimeState::Running;
        Ok(())
    }

    /// 优雅关闭
    pub async fn shutdown(&self) -> Result<(), String> {
        let mut state = self.state.write().await;
        *state = RuntimeState::Stopping;
        // TODO: 关闭各子系统
        *state = RuntimeState::Stopped;
        Ok(())
    }

    /// 是否正在运行
    pub async fn is_running(&self) -> bool {
        *self.state.read().await == RuntimeState::Running
    }
}