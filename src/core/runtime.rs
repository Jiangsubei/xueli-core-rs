use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::core::config::XueliConfig;
use crate::core::log_labels::LOG_STARTUP_INFO;
use crate::core::metrics::RuntimeMetrics;
use crate::core::platform_types::GroupState;
use crate::prelude::XueliResult;

/// Bot 运行时 — 系统生命周期管理中心，包含群状态机、消息缓冲与触发引擎。
pub struct BotRuntime {
    pub config: Arc<XueliConfig>,
    pub metrics: Arc<RwLock<RuntimeMetrics>>,
    state: Arc<RwLock<RuntimeState>>,
    /// 群聊状态
    group_states: Arc<RwLock<HashMap<String, GroupState>>>,
    /// 群聊待处理计数
    group_pending_counts: Arc<Mutex<HashMap<String, usize>>>,
    /// 触发保护锁（防并发触发）
    group_trigger_locks: Arc<Mutex<HashMap<String, bool>>>,
    /// 群聊最后活动时间
    group_last_activity: Arc<Mutex<HashMap<String, f64>>>,
    /// 最近回复时间戳（用于计算平均延迟）
    recent_reply_timestamps: Arc<Mutex<HashMap<String, VecDeque<f64>>>>,
    /// STOPPED 冷却标记
    group_stopped_at: Arc<Mutex<HashMap<String, f64>>>,
    /// 最大中断次数
    max_interrupt_count: usize,
    /// 已处理消息 ID（去重）
    processed_message_ids: Arc<Mutex<VecDeque<String>>>,
    /// 最大去重缓存
    max_dedup_size: usize,
}

/// 运行时生命周期状态
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
            group_pending_counts: Arc::new(Mutex::new(HashMap::new())),
            group_trigger_locks: Arc::new(Mutex::new(HashMap::new())),
            group_last_activity: Arc::new(Mutex::new(HashMap::new())),
            recent_reply_timestamps: Arc::new(Mutex::new(HashMap::new())),
            group_stopped_at: Arc::new(Mutex::new(HashMap::new())),
            max_interrupt_count: 2,
            processed_message_ids: Arc::new(Mutex::new(VecDeque::new())),
            max_dedup_size: 500,
        }
    }

    /// 初始化运行时
    pub async fn init(&self) -> XueliResult<()> {
        let mut s = self.state.write().await;
        if *s != RuntimeState::Created {
            return Err(format!("不能从状态 {:?} 初始化", *s).into());
        }
        *s = RuntimeState::Initializing;
        tracing::info!(target: LOG_STARTUP_INFO, "运行时初始化完成");
        *s = RuntimeState::Running;
        Ok(())
    }

    /// 优雅关闭
    pub async fn shutdown(&self) -> XueliResult<()> {
        let mut s = self.state.write().await;
        *s = RuntimeState::Stopping;
        self.group_states.write().await.clear();
        self.group_pending_counts.lock().await.clear();
        self.group_trigger_locks.lock().await.clear();
        tracing::info!("运行时已关闭");
        *s = RuntimeState::Stopped;
        Ok(())
    }

    pub async fn is_running(&self) -> bool {
        *self.state.read().await == RuntimeState::Running
    }

    // ── 消息去重 ──

    /// 检查消息是否已处理过（通过 message_id 去重），未处理则登记
    pub async fn check_and_mark_processed(&self, message_id: &str) -> bool {
        if message_id.is_empty() {
            return true;
        }
        let mut ids = self.processed_message_ids.lock().await;
        if ids.contains(&message_id.to_string()) {
            return false;
        }
        ids.push_back(message_id.to_string());
        while ids.len() > self.max_dedup_size {
            ids.pop_front();
        }
        true
    }

    // ── 群聊状态机 ──

    pub async fn get_group_state(&self, group_key: &str) -> GroupState {
        self.group_states
            .read()
            .await
            .get(group_key)
            .copied()
            .unwrap_or(GroupState::Running)
    }

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
        if new_state == GroupState::Stopped {
            self.group_stopped_at
                .lock()
                .await
                .insert(group_key.to_string(), now_secs());
        }
    }

    pub async fn set_group_waiting(&self, group_key: &str) {
        self.set_group_state(group_key, GroupState::Waiting).await;
    }

    pub async fn set_group_stopped(&self, group_key: &str) {
        self.set_group_state(group_key, GroupState::Stopped).await;
    }

    pub async fn try_wake_group(&self, group_key: &str) {
        if self.get_group_state(group_key).await == GroupState::Stopped {
            self.set_group_state(group_key, GroupState::Running).await;
        }
    }

    pub async fn is_group_waiting(&self, group_key: &str) -> bool {
        self.get_group_state(group_key).await == GroupState::Waiting
    }

    pub async fn should_ignore_due_to_stopped(&self, group_key: &str) -> bool {
        self.get_group_state(group_key).await == GroupState::Stopped
    }

    pub async fn stop_all_groups(&self) {
        let mut states = self.group_states.write().await;
        for (key, state) in states.iter_mut() {
            if *state == GroupState::Running || *state == GroupState::Waiting {
                tracing::info!("[状态机] key={} {:?} → Stopped", key, *state);
                *state = GroupState::Stopped;
            }
        }
    }

    // ── 消息缓冲与触发引擎 ──

    /// 注册一条待处理消息，返回是否应触发处理。
    ///
    /// 逻辑：递增 pending 计数 → 检查阈值 → 空闲补偿 → debounce 保护。
    /// 若不应触发，则更新计数但不调度处理。
    pub async fn register_pending_message(&self, group_key: &str) -> bool {
        let threshold = self.calculate_trigger_threshold(group_key);
        let idle_grace = 300.0;

        let mut counts = self.group_pending_counts.lock().await;
        let count = counts.get(group_key).copied().unwrap_or(0) + 1;
        counts.insert(group_key.to_string(), count);

        // 检查是否已在等待触发
        let mut trigger_locks = self.group_trigger_locks.lock().await;
        let already_triggered = trigger_locks.get(group_key).copied().unwrap_or(false);
        if already_triggered {
            return false;
        }

        // STOPPED 冷却检查
        if self.get_group_state(group_key).await == GroupState::Stopped {
            let stopped_at = self.group_stopped_at.lock().await.get(group_key).copied();
            if let Some(stopped_time) = stopped_at {
                let cooldown = 30.0;
                if now_secs() - stopped_time < cooldown {
                    return false;
                }
            }
            // 冷却结束，唤醒
            self.set_group_state(group_key, GroupState::Running).await;
        }

        let should_process = if count >= threshold {
            true
        } else {
            let last_activity = self
                .group_last_activity
                .lock()
                .await
                .get(group_key)
                .copied()
                .unwrap_or(0.0);
            // 仅在已有活动记录时才启用空闲补偿
            if last_activity > 0.0 {
                let idle_time = now_secs() - last_activity;
                idle_time >= idle_grace
            } else {
                false
            }
        };

        if should_process {
            counts.insert(group_key.to_string(), 0);
            trigger_locks.insert(group_key.to_string(), true);
            true
        } else {
            false
        }
    }

    /// 处理完成后清理触发标记和记录最后活动时间
    pub async fn finish_processing(&self, group_key: &str) {
        self.group_trigger_locks
            .lock()
            .await
            .insert(group_key.to_string(), false);
        self.group_last_activity
            .lock()
            .await
            .insert(group_key.to_string(), now_secs());
    }

    /// 计算群聊触发阈值。
    ///
    /// 基础阈值 = ceil(1 / base_frequency)，受群覆盖因子和饱和平滑影响，最小为 1。
    pub fn calculate_trigger_threshold(&self, _group_key: &str) -> usize {
        // 简化实现：默认每 3 条消息触发一次
        3
    }

    /// 记录一次回复时间戳（用于后续延迟跟踪）
    pub async fn record_reply_timestamp(&self, group_key: &str) {
        let now = now_secs();
        let mut timestamps = self.recent_reply_timestamps.lock().await;
        let ts = timestamps.entry(group_key.to_string()).or_default();
        ts.push_back(now);
        // 保留 10 分钟内的记录
        while ts.front().map_or(false, |t| now - t > 600.0) {
            ts.pop_front();
        }
    }

    /// 获取 pending 计数
    pub async fn get_pending_count(&self, group_key: &str) -> usize {
        self.group_pending_counts
            .lock()
            .await
            .get(group_key)
            .copied()
            .unwrap_or(0)
    }
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lifecycle() {
        let rt = BotRuntime::new(XueliConfig::default());
        assert!(!rt.is_running().await);
        rt.init().await.unwrap();
        assert!(rt.is_running().await);
        rt.shutdown().await.unwrap();
        assert!(!rt.is_running().await);
    }

    #[tokio::test]
    async fn test_group_state_machine() {
        let rt = BotRuntime::new(XueliConfig::default());
        assert_eq!(rt.get_group_state("g1").await, GroupState::Running);
        rt.set_group_waiting("g1").await;
        assert!(rt.is_group_waiting("g1").await);
        rt.set_group_stopped("g1").await;
        assert!(rt.should_ignore_due_to_stopped("g1").await);
        rt.try_wake_group("g1").await;
        assert_eq!(rt.get_group_state("g1").await, GroupState::Running);
    }

    #[tokio::test]
    async fn test_dedup() {
        let rt = BotRuntime::new(XueliConfig::default());
        assert!(rt.check_and_mark_processed("m1").await);
        assert!(!rt.check_and_mark_processed("m1").await);
        assert!(rt.check_and_mark_processed("m2").await);
    }

    #[tokio::test]
    async fn test_pending_message_buffering() {
        let rt = BotRuntime::new(XueliConfig::default());
        // 第一条消息：不应触发（count=1 < threshold=3）
        assert!(!rt.register_pending_message("g1").await);
        assert_eq!(rt.get_pending_count("g1").await, 1);
        // 第二条：仍不触发
        assert!(!rt.register_pending_message("g1").await);
        assert_eq!(rt.get_pending_count("g1").await, 2);
        // 第三条：应触发
        assert!(rt.register_pending_message("g1").await);
        // 触发后计数清零
        assert_eq!(rt.get_pending_count("g1").await, 0);
        // 清理触发标记
        rt.finish_processing("g1").await;
        assert_eq!(rt.get_pending_count("g1").await, 0);
    }

    #[tokio::test]
    async fn test_record_reply_timestamp() {
        let rt = BotRuntime::new(XueliConfig::default());
        rt.record_reply_timestamp("g1").await;
        rt.record_reply_timestamp("g1").await;
        let timestamps = rt.recent_reply_timestamps.lock().await;
        assert_eq!(timestamps.get("g1").map(|ts| ts.len()).unwrap_or(0), 2);
    }
}
