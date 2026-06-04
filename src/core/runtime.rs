use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::prelude::XueliResult;
use crate::core::config::XueliConfig;
use crate::core::log_labels::LOG_STARTUP_INFO;
use crate::core::metrics::RuntimeMetrics;
use crate::core::platform_types::GroupState;

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
    /// 群聊状态（group_key → GroupState）
    group_states: Arc<RwLock<HashMap<String, GroupState>>>,
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
            group_states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 初始化运行时
    pub async fn init(&self) -> XueliResult<()> {
        let mut state = self.state.write().await;
        if *state != RuntimeState::Created {
            return Err(format!("不能从状态 {:?} 初始化", *state).into());
        }
        *state = RuntimeState::Initializing;

        tracing::info!(target: LOG_STARTUP_INFO, "运行时初始化完成");

        *state = RuntimeState::Running;
        Ok(())
    }

    /// 优雅关闭
    pub async fn shutdown(&self) -> XueliResult<()> {
        let mut state = self.state.write().await;
        *state = RuntimeState::Stopping;

        // 清理所有群聊状态
        self.group_states.write().await.clear();

        tracing::info!("运行时已关闭");
        *state = RuntimeState::Stopped;
        Ok(())
    }

    /// 是否正在运行
    pub async fn is_running(&self) -> bool {
        *self.state.read().await == RuntimeState::Running
    }

    // ── 群聊状态机 ──

    /// 获取群聊状态
    pub async fn get_group_state(&self, group_key: &str) -> GroupState {
        let states = self.group_states.read().await;
        states
            .get(group_key)
            .copied()
            .unwrap_or(GroupState::Running)
    }

    /// 设置群聊状态
    pub async fn set_group_state(&self, group_key: &str, new_state: GroupState) {
        let mut states = self.group_states.write().await;
        let old = states.get(group_key).copied();
        states.insert(group_key.to_string(), new_state);
        if old != Some(new_state) {
            tracing::info!(
                "[状态机] key={} {:?} → {:?}",
                group_key,
                old.unwrap_or(GroupState::Running),
                new_state
            );
        }
    }

    /// 将群聊设置为 WAITING（缓冲消息，等待更多上下文）
    pub async fn set_group_waiting(&self, group_key: &str) {
        self.set_group_state(group_key, GroupState::Waiting).await;
    }

    /// 将群聊设置为 STOPPED（忽略消息）
    pub async fn set_group_stopped(&self, group_key: &str) {
        self.set_group_state(group_key, GroupState::Stopped).await;
    }

    /// 尝试从 STOPPED 恢复为 RUNNING
    pub async fn try_wake_group(&self, group_key: &str) {
        let current = self.get_group_state(group_key).await;
        if current == GroupState::Stopped {
            self.set_group_state(group_key, GroupState::Running).await;
        }
    }

    /// 是否是群聊且处于 WAITING 状态
    pub async fn is_group_waiting(&self, group_key: &str) -> bool {
        self.get_group_state(group_key).await == GroupState::Waiting
    }

    /// 是否由于已停止而应忽略消息
    pub async fn should_ignore_due_to_stopped(&self, group_key: &str) -> bool {
        self.get_group_state(group_key).await == GroupState::Stopped
    }

    /// 停止所有群聊（用于全局暂停场景）
    pub async fn stop_all_groups(&self) {
        let mut states = self.group_states.write().await;
        for (key, state) in states.iter_mut() {
            if *state == GroupState::Running || *state == GroupState::Waiting {
                tracing::info!("[状态机] key={} {:?} → Stopped", key, *state);
                *state = GroupState::Stopped;
            }
        }
    }
}
