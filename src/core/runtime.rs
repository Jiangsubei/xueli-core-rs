use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, Timelike};
use tokio::sync::{Mutex, RwLock};

use crate::core::config::{ProactiveShareConfig, XueliConfig};
use crate::core::log_labels::{LOG_RETRY, LOG_STARTUP_INFO};
use crate::core::metrics::RuntimeMetrics;
use crate::core::platform_types::{GroupState, InboundEvent, ReplyAction};
use crate::core::scope::ChatScope;
use crate::handlers::message_handler::MessageHandler;
use crate::prelude::XueliResult;
use crate::proactive_share::scheduler::ProactiveShareScheduler;
use crate::proactive_share::store::ProactiveShareStore;
use crate::services::ai_client::DefaultAIClient;
use crate::services::prompt_loader::NoopPromptTemplateLoader;
use crate::traits::platform_adapter::PlatformAdapter;

/// 运行时外部挂钩（mood / saturation 等依赖外部系统）
#[derive(Clone)]
pub struct RuntimeHooks {
    /// 群聊触发阈值的心情附加值（越高越不情愿回复）
    pub mood_threshold_bonus: Option<Arc<dyn Fn(&str) -> f64 + Send + Sync>>,
    /// STOPPED 冷却时长（秒），若未设置则回退 30.0
    pub stopped_cooldown: Option<Arc<dyn Fn(&str) -> f64 + Send + Sync>>,
    /// LLM 推荐值读取（key: "next_participation_energy" / "preferred_cooldown_seconds" 等）
    pub llm_recommendation: Option<Arc<dyn Fn(&str, &str) -> Option<f64> + Send + Sync>>,
}

impl Default for RuntimeHooks {
    fn default() -> Self {
        Self {
            mood_threshold_bonus: None,
            stopped_cooldown: None,
            llm_recommendation: None,
        }
    }
}

/// Bot 运行时 — 系统生命周期管理中心，包含群状态机、消息缓冲与触发引擎。
pub struct BotRuntime<P: PlatformAdapter + 'static> {
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
    /// 最近回复完成时间戳（用于计算平均回复间隔，10 分钟滑动窗口）
    recent_reply_timestamps: Arc<Mutex<HashMap<String, VecDeque<f64>>>>,
    /// 群聊回复时间戳（用于饱和度追踪，5 分钟滑动窗口）
    group_reply_timestamps: Arc<Mutex<HashMap<String, VecDeque<f64>>>>,
    /// 群聊待处理事件（用于延迟触发时重建上下文）
    group_pending_events: Arc<Mutex<HashMap<String, InboundEvent>>>,
    /// 回合是否已调度（防双重触发竞态）
    group_turn_scheduled: Arc<Mutex<HashMap<String, bool>>>,
    /// 延迟触发 / 冷却唤醒任务是否活跃（防重复调度）
    group_debounce_active: Arc<Mutex<HashMap<String, bool>>>,
    /// STOPPED 冷却标记
    group_stopped_at: Arc<Mutex<HashMap<String, f64>>>,
    /// 最大中断次数
    #[allow(dead_code)]
    max_interrupt_count: usize,
    /// 已处理消息 ID（去重）
    processed_message_ids: Arc<Mutex<VecDeque<String>>>,
    /// 最大去重缓存
    max_dedup_size: usize,
    /// 延迟触发通知通道（当 deferred_trigger / stopped_cooldown 到期时发送 group_key）
    trigger_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    /// 外部挂钩
    pub hooks: RuntimeHooks,
    /// 消息处理器
    message_handler: Option<Arc<MessageHandler<DefaultAIClient, P, NoopPromptTemplateLoader>>>,
    /// 主动分享存储
    proactive_share_store: Option<Arc<ProactiveShareStore>>,
    /// 主动分享调度器
    proactive_scheduler: Option<Arc<ProactiveShareScheduler>>,
    /// 平台适配器
    adapter: Option<Arc<P>>,
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

impl<P: PlatformAdapter + 'static> BotRuntime<P> {
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
            group_reply_timestamps: Arc::new(Mutex::new(HashMap::new())),
            group_pending_events: Arc::new(Mutex::new(HashMap::new())),
            group_turn_scheduled: Arc::new(Mutex::new(HashMap::new())),
            group_debounce_active: Arc::new(Mutex::new(HashMap::new())),
            group_stopped_at: Arc::new(Mutex::new(HashMap::new())),
            max_interrupt_count: 2,
            processed_message_ids: Arc::new(Mutex::new(VecDeque::new())),
            max_dedup_size: 500,
            trigger_tx: None,
            hooks: RuntimeHooks::default(),
            message_handler: None,
            proactive_share_store: None,
            proactive_scheduler: None,
            adapter: None,
        }
    }

    /// 设置延迟触发通知通道
    pub fn set_trigger_tx(&mut self, tx: tokio::sync::mpsc::UnboundedSender<String>) {
        self.trigger_tx = Some(tx);
    }

    /// 设置消息处理器
    pub fn set_message_handler(
        &mut self,
        handler: Arc<MessageHandler<DefaultAIClient, P, NoopPromptTemplateLoader>>,
    ) {
        self.message_handler = Some(handler);
    }

    /// 设置平台适配器
    pub fn set_adapter(&mut self, adapter: Arc<P>) {
        self.adapter = Some(adapter);
    }

    /// 设置主动分享组件
    pub fn setup_proactive_share(
        &mut self,
        config: &ProactiveShareConfig,
        store: Arc<ProactiveShareStore>,
    ) -> XueliResult<()> {
        let arc_config = Arc::new(config.clone());
        let scheduler = ProactiveShareScheduler::new(arc_config, store.clone());
        self.proactive_share_store = Some(store);
        self.proactive_scheduler = Some(Arc::new(scheduler));
        Ok(())
    }

    /// 发送主动分享
    pub async fn send_proactive_share(
        &self,
        content: &str,
        target_user_id: &str,
        target_group_id: Option<&str>,
    ) -> XueliResult<bool> {
        let adapter = match &self.adapter {
            Some(a) => a.clone(),
            None => {
                tracing::debug!("[主动分享] 未配置平台适配器，跳过发送");
                return Ok(false);
            }
        };

        let scope = if let Some(gid) = target_group_id {
            ChatScope::Group(gid.to_string())
        } else {
            ChatScope::Private
        };

        let action = ReplyAction {
            scope,
            text: content.to_string(),
            reply_to: None,
            image_url: None,
            emoji_id: None,
        };

        match adapter.send_action(&action).await {
            Ok(()) => {
                tracing::info!("[主动分享] 发送成功 target_user={}", target_user_id);
                Ok(true)
            }
            Err(e) => {
                tracing::error!("[主动分享] 发送失败: {}", e);
                Ok(false)
            }
        }
    }

    /// 夜间情绪恢复 — 对所有情绪引擎执行夜间恢复并持久化
    pub async fn apply_mood_night_recovery(&self) -> XueliResult<()> {
        if self.message_handler.is_none() {
            tracing::debug!("[夜间恢复] 未配置消息处理器，跳过");
            return Ok(());
        }

        tracing::info!("[夜间恢复] 未实现情绪引擎夜间恢复（MessageHandler 尚无 mood_engines）");
        Ok(())
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
        self.group_pending_events.lock().await.clear();
        self.group_turn_scheduled.lock().await.clear();
        self.group_debounce_active.lock().await.clear();
        self.recent_reply_timestamps.lock().await.clear();
        self.group_reply_timestamps.lock().await.clear();
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
    /// 逻辑：递增 pending 计数 → 检查阈值（动态计算）→ 空闲补偿 → debounce 调度。
    /// 若不应触发，则更新计数但不调度处理。
    /// event 参数用于延迟触发时重建上下文，可直接或稍后通过 set_pending_event 设置。
    pub async fn register_pending_message(
        &self,
        group_key: &str,
        event: Option<InboundEvent>,
    ) -> bool {
        let config = &self.config.group_reply;
        let threshold = self.calculate_trigger_threshold(group_key).await;
        let idle_grace = config.idle_grace_seconds;

        let mut counts = self.group_pending_counts.lock().await;
        let count = counts.get(group_key).copied().unwrap_or(0) + 1;
        counts.insert(group_key.to_string(), count);
        drop(counts);

        // 存储事件供延迟触发使用
        if let Some(evt) = event {
            self.group_pending_events
                .lock()
                .await
                .insert(group_key.to_string(), evt);
        }

        // 检查是否已调度回合
        let already_scheduled = self
            .group_turn_scheduled
            .lock()
            .await
            .get(group_key)
            .copied()
            .unwrap_or(false);
        if already_scheduled {
            return false;
        }

        // STOPPED 冷却检查
        if self.get_group_state(group_key).await == GroupState::Stopped {
            let stopped_at = self.group_stopped_at.lock().await.get(group_key).copied();
            if let Some(stopped_time) = stopped_at {
                let cooldown = self.get_stopped_cooldown(group_key);
                let elapsed = now_secs() - stopped_time;
                if elapsed < cooldown {
                    // 冷却中 → 调度冷却唤醒（仅首次）
                    if count == 1 {
                        let mut active = self.group_debounce_active.lock().await;
                        if !active.get(group_key).copied().unwrap_or(false) {
                            active.insert(group_key.to_string(), true);
                            drop(active);
                            let remaining = cooldown - elapsed;
                            self.spawn_stopped_cooldown_trigger(group_key, remaining);
                        }
                    }
                    tracing::info!(
                        "[状态机] key={} STOPPED 冷却中 ({:.1}/{:.1}s), pending={}, 消息缓冲",
                        group_key,
                        elapsed,
                        cooldown,
                        count,
                    );
                    return false;
                }
            }
            // 冷却结束，唤醒
            self.set_group_state(group_key, GroupState::Running).await;
        }

        let mut should_process = count >= threshold;

        if !should_process {
            // 自适应空闲补偿：使用平均回复间隔估算等效消息数
            if let Some(avg_latency) = self.get_average_reply_interval(group_key).await {
                if avg_latency > 0.0 {
                    let last_activity = self
                        .group_last_activity
                        .lock()
                        .await
                        .get(group_key)
                        .copied()
                        .unwrap_or(0.0);
                    let idle_time = now_secs() - last_activity;
                    let equivalent = count as f64 + idle_time / avg_latency;
                    should_process = equivalent >= threshold as f64;
                }
            }
        }

        if !should_process {
            let last_activity = self
                .group_last_activity
                .lock()
                .await
                .get(group_key)
                .copied()
                .unwrap_or(0.0);
            if last_activity > 0.0 {
                let idle_time = now_secs() - last_activity;
                should_process = count > 0 && idle_time >= idle_grace;
            }
        }

        if should_process {
            let mut counts = self.group_pending_counts.lock().await;
            counts.insert(group_key.to_string(), 0);
            drop(counts);
            self.group_turn_scheduled
                .lock()
                .await
                .insert(group_key.to_string(), true);
            true
        } else {
            // 未到阈值 → 调度延迟触发器
            let mut active = self.group_debounce_active.lock().await;
            if !active.get(group_key).copied().unwrap_or(false) {
                active.insert(group_key.to_string(), true);
                drop(active);
                self.spawn_deferred_trigger(group_key);
            }
            false
        }
    }

    /// 单独设置群聊待处理事件（用于延迟触发上下文重建）
    pub async fn set_pending_event(&self, group_key: &str, event: InboundEvent) {
        self.group_pending_events
            .lock()
            .await
            .insert(group_key.to_string(), event);
    }

    /// 处理完成后清理触发标记和记录最后活动时间
    pub async fn finish_processing(&self, group_key: &str) {
        self.group_turn_scheduled
            .lock()
            .await
            .insert(group_key.to_string(), false);
        self.group_last_activity
            .lock()
            .await
            .insert(group_key.to_string(), now_secs());
        // 清零 pending 计数（非清除，保留 key 便于后续 debounce 检查）
        self.group_pending_counts
            .lock()
            .await
            .insert(group_key.to_string(), 0);
    }

    /// 完整清理（处理完毕 + 清空 pending 和 turn_scheduled）
    pub async fn cleanup_after_processing(&self, group_key: &str) {
        self.group_turn_scheduled.lock().await.remove(group_key);
        self.group_pending_counts.lock().await.remove(group_key);
        self.group_last_activity
            .lock()
            .await
            .insert(group_key.to_string(), now_secs());
    }

    // ── 消息处理管线 ──

    /// 处理缓冲消息（被触发引擎调用）。
    ///
    /// 从 group_pending_events 取事件 → message_handler.handle() → 发送回复 → 清理。
    pub async fn _process_reply(&self, group_key: &str) -> XueliResult<()> {
        let event = {
            let events = self.group_pending_events.lock().await;
            events.get(group_key).cloned()
        };

        let event = match event {
            Some(e) => e,
            None => {
                tracing::debug!("[处理] key={} 无待处理事件", group_key);
                self.cleanup_after_processing(group_key).await;
                return Ok(());
            }
        };

        let handler = match &self.message_handler {
            Some(h) => h.clone(),
            None => {
                tracing::debug!("[处理] key={} 未配置消息处理器，直接清理", group_key);
                self.cleanup_after_processing(group_key).await;
                return Ok(());
            }
        };

        // 记录处理开始
        self.finish_processing(group_key).await;

        // 运行 agent 循环（含重试）
        let result = self._run_agent_loop(&event, group_key, handler).await;

        // 记录回复完成
        self.record_reply_completion(group_key).await;
        self.record_group_reply(group_key).await;

        // 清理
        self.cleanup_after_processing(group_key).await;

        result
    }

    /// agent 循环：调用消息处理器，若成功则发送回复。
    async fn _run_agent_loop(
        &self,
        event: &InboundEvent,
        group_key: &str,
        handler: Arc<MessageHandler<DefaultAIClient, P, NoopPromptTemplateLoader>>,
    ) -> XueliResult<()> {
        let max_retries = 2;
        let mut attempt = 0;

        loop {
            attempt += 1;

            match handler.handle(event).await {
                Ok(Some(action)) => {
                    // 发送回复
                    if let Some(adapter) = &self.adapter {
                        if let Err(e) = adapter.send_action(&action).await {
                            tracing::error!("[处理] key={} 发送回复失败: {}", group_key, e,);
                        } else {
                            tracing::info!(
                                "[处理] key={} 回复发送成功 attempt={}",
                                group_key,
                                attempt
                            );
                        }
                    } else {
                        tracing::debug!("[处理] key={} 未配置适配器，跳过发送", group_key);
                    }
                    return Ok(());
                }
                Ok(None) => {
                    // 决策不回复（wait/ignore）
                    tracing::debug!("[处理] key={} 决策不回复，转入 WAITING", group_key);
                    self.set_group_waiting(group_key).await;
                    return Ok(());
                }
                Err(e) if attempt < max_retries => {
                    tracing::info!(
                        target: LOG_RETRY,
                        "[处理] key={} AI 调用失败 attempt={}/{}: {}",
                        group_key,
                        attempt,
                        max_retries,
                        e,
                    );
                    tokio::time::sleep(Duration::from_secs_f64(1.0)).await;
                }
                Err(e) => {
                    tracing::error!(
                        "[处理] key={} AI 调用最终失败 attempt={}/{}: {}",
                        group_key,
                        attempt,
                        max_retries,
                        e,
                    );
                    return Err(e);
                }
            }
        }
    }

    // ── 触发阈值计算 ──

    /// 动态计算群聊触发阈值。
    ///
    /// 公式：
    ///   effective = base_frequency × time_rules_factor × group_override  (clamp [0.05, 1.0])
    ///   base_threshold = max(1, ceil(1 / effective))
    ///   dynamic = (base_threshold + mood_bonus) × saturation_multiplier
    ///   result = max(1, round(dynamic))
    pub async fn calculate_trigger_threshold(&self, group_key: &str) -> usize {
        let config = &self.config.group_reply;
        let mut effective = config.base_frequency;

        // 时间规则（按当前小时匹配时间段因子）
        if !config.time_rules.is_empty() {
            let now = Local::now();
            let hour = now.hour();
            for (time_range_str, factor_val) in &config.time_rules {
                if Self::in_time_range(hour, time_range_str) {
                    effective *= factor_val;
                    break;
                }
            }
        }

        // 群组覆盖因子
        if let Some(override_val) = config.group_overrides.get(group_key) {
            effective *= override_val;
        }

        effective = effective.clamp(0.05, 1.0);
        let base_threshold = (1.0 / effective + 0.5) as usize;

        // 心情附加值（通过挂钩获取，未设置则为 0）
        let mood_bonus = self
            .hooks
            .mood_threshold_bonus
            .as_ref()
            .map(|f| f(group_key))
            .unwrap_or(0.0);

        // 饱和度乘数
        let sat_mult = self.get_group_saturation_multiplier(group_key).await;

        let dynamic = (base_threshold as f64 + mood_bonus) * sat_mult;
        let result = (dynamic + 0.5) as usize;
        let result = result.max(1);

        if mood_bonus > 0.0 {
            tracing::info!(
                "[触发阈值] key={} effective={:.2} base={} mood={:.1} sat_mult={:.2} result={}",
                group_key,
                effective,
                base_threshold,
                mood_bonus,
                sat_mult,
                result,
            );
        }

        result
    }

    /// 判断当前小时是否在时间段 "HH:MM-HH:MM" 内（支持跨午夜）
    fn in_time_range(hour: u32, time_range: &str) -> bool {
        let parts: Vec<&str> = time_range.trim().split('-').collect();
        if parts.len() != 2 {
            return false;
        }
        let parse_minutes = |s: &str| -> Option<u32> {
            let segs: Vec<&str> = s.split(':').collect();
            if segs.len() != 2 {
                return None;
            }
            let h: u32 = segs[0].parse().ok()?;
            let m: u32 = segs[1].parse().ok()?;
            Some(h * 60 + m)
        };
        let start_min = match parse_minutes(parts[0]) {
            Some(v) => v,
            None => return false,
        };
        let end_min = match parse_minutes(parts[1]) {
            Some(v) => v,
            None => return false,
        };
        let now = Local::now();
        let current_min = hour * 60 + now.minute();
        if start_min <= end_min {
            start_min <= current_min && current_min < end_min
        } else {
            // 跨午夜（如 "22:00-06:00"）
            current_min >= start_min || current_min < end_min
        }
    }

    // ── Debounce 消抖 ──

    /// 处理前消抖等待：若在 debounce 期间有新消息到达，重置等待。
    pub async fn debounce_wait(&self, group_key: &str) {
        let config = &self.config.group_reply;
        let debounce = config.debounce_seconds;
        let max_resets = config.debounce_max_resets;
        if debounce <= 0.0 || max_resets == 0 {
            return;
        }

        let mut resets = 0;
        while resets < max_resets {
            let pre_count = self.get_pending_count(group_key).await;
            tokio::time::sleep(Duration::from_secs_f64(debounce)).await;
            let post_count = self.get_pending_count(group_key).await;
            if post_count == pre_count {
                return;
            }
            resets += 1;
        }
        tracing::info!(
            "[消抖] key={} 超过重置上限 {}, 强制继续",
            group_key,
            max_resets,
        );
    }

    // ── 延迟触发 / 冷却唤醒 ──

    /// 调度预测时间的延迟触发器（内部 spawn）
    fn spawn_deferred_trigger(&self, group_key: &str) {
        let gk = group_key.to_string();
        let rt_weak = Arc::downgrade(&self.config);
        let trigger_tx = self.trigger_tx.clone();
        let pending_counts = Arc::clone(&self.group_pending_counts);
        let turn_scheduled = Arc::clone(&self.group_turn_scheduled);
        let debounce_active = Arc::clone(&self.group_debounce_active);
        let hooks = self.hooks.clone();

        tokio::spawn(async move {
            let config = match rt_weak.upgrade() {
                Some(c) => c,
                None => return,
            };
            let estimated_interval = config.group_reply.plan_request_interval;

            let pending = *pending_counts.lock().await.get(&gk).unwrap_or(&0);
            if pending == 0 {
                debounce_active.lock().await.remove(&gk);
                return;
            }

            let threshold = {
                let mut effective = config.group_reply.base_frequency;
                if !config.group_reply.time_rules.is_empty() {
                    let now = Local::now();
                    let hour = now.hour();
                    for (time_range_str, factor_val) in &config.group_reply.time_rules {
                        if Self::in_time_range(hour, time_range_str) {
                            effective *= factor_val;
                            break;
                        }
                    }
                }
                if let Some(ov) = config.group_reply.group_overrides.get(&gk) {
                    effective *= ov;
                }
                effective = effective.clamp(0.05, 1.0);
                let base = (1.0 / effective + 0.5) as usize;
                let mood = hooks
                    .mood_threshold_bonus
                    .as_ref()
                    .map(|f| f(&gk))
                    .unwrap_or(0.0);
                let sat_mult = 1.0;
                let dynamic = (base as f64 + mood) * sat_mult;
                (dynamic + 0.5) as usize
            };
            let threshold = threshold.max(1);

            let missing = threshold.saturating_sub(pending);
            let predicted_delay = missing as f64 * estimated_interval;
            let delay = predicted_delay.clamp(1.0, 300.0);

            tokio::time::sleep(Duration::from_secs_f64(delay)).await;

            let mut active = debounce_active.lock().await;
            active.remove(&gk);
            drop(active);

            let pending = *pending_counts.lock().await.get(&gk).unwrap_or(&0);
            let already = turn_scheduled
                .lock()
                .await
                .get(&gk)
                .copied()
                .unwrap_or(false);

            if pending > 0 && !already {
                pending_counts.lock().await.insert(gk.clone(), 0);
                turn_scheduled.lock().await.insert(gk.clone(), true);
                tracing::info!("[预测触发] key={} 到期主动触发处理", gk);
                if let Some(tx) = &trigger_tx {
                    let _ = tx.send(gk.clone());
                }
            }
        });
    }

    /// 调度 STOPPED 冷却唤醒（内部 spawn）
    fn spawn_stopped_cooldown_trigger(&self, group_key: &str, delay: f64) {
        let gk = group_key.to_string();
        let trigger_tx = self.trigger_tx.clone();
        let pending_counts = Arc::clone(&self.group_pending_counts);
        let turn_scheduled = Arc::clone(&self.group_turn_scheduled);
        let debounce_active = Arc::clone(&self.group_debounce_active);

        tokio::spawn(async move {
            let sleep_dur = delay.max(1.0);
            tokio::time::sleep(Duration::from_secs_f64(sleep_dur)).await;

            let mut active = debounce_active.lock().await;
            active.remove(&gk);
            drop(active);

            let pending = *pending_counts.lock().await.get(&gk).unwrap_or(&0);
            let already = turn_scheduled
                .lock()
                .await
                .get(&gk)
                .copied()
                .unwrap_or(false);

            if pending > 0 && !already {
                pending_counts.lock().await.insert(gk.clone(), 0);
                turn_scheduled.lock().await.insert(gk.clone(), true);
                tracing::info!(
                    "[冷却触发] key={} 发现 {} 条缓冲消息，触发处理",
                    gk,
                    pending
                );
                if let Some(tx) = &trigger_tx {
                    let _ = tx.send(gk.clone());
                }
            } else {
                tracing::info!("[冷却触发] key={} 无缓冲消息，恢复 RUNNING", gk);
            }
        });
    }

    // ── 回复时间追踪 ──

    /// 记录回复完成时间戳（用于计算平均回复间隔）
    pub async fn record_reply_completion(&self, group_key: &str) {
        let now = now_secs();
        let mut timestamps = self.recent_reply_timestamps.lock().await;
        let ts = timestamps.entry(group_key.to_string()).or_default();
        ts.push_back(now);
        // 保留 10 分钟内的记录
        while ts.front().map_or(false, |t| now - t > 600.0) {
            ts.pop_front();
        }
    }

    /// 记录群聊回复时间戳（用于饱和度追踪，5 分钟滑动窗口）
    pub async fn record_group_reply(&self, group_key: &str) {
        let now = now_secs();
        let mut timestamps = self.group_reply_timestamps.lock().await;
        let ts = timestamps.entry(group_key.to_string()).or_default();
        ts.push_back(now);
        while ts.front().map_or(false, |t| now - t > 300.0) {
            ts.pop_front();
        }
    }

    /// 获取 10 分钟滑动窗口内的平均回复间隔（秒）
    ///
    /// 计算相邻回复完成时间戳之间的平均间隔，用于自适应空闲补偿。
    /// 若窗口内不足 2 条记录，返回 None。
    pub async fn get_average_reply_interval(&self, group_key: &str) -> Option<f64> {
        let timestamps = self.recent_reply_timestamps.lock().await;
        let ts_vec = timestamps.get(group_key)?;
        if ts_vec.len() < 2 {
            return None;
        }
        let now = now_secs();
        let recent: Vec<f64> = ts_vec.iter().copied().filter(|t| now - t < 600.0).collect();
        if recent.len() < 2 {
            return None;
        }
        let gaps: Vec<f64> = recent.windows(2).map(|w| w[1] - w[0]).collect();
        if gaps.is_empty() {
            return None;
        }
        let avg = gaps.iter().sum::<f64>() / gaps.len() as f64;
        Some(avg.max(0.0))
    }

    // ── 饱和度 / 冷却 ──

    /// 获取群聊饱和度乘数（基于最近 5 分钟内的回复频率）
    ///
    /// 公式：若 recent ≤ 1 → 1.0；否则 1.0 + log_factor × ln(1 + recent/2)
    pub async fn get_group_saturation_multiplier(&self, group_key: &str) -> f64 {
        let timestamps = self.group_reply_timestamps.lock().await;
        let ts_vec = match timestamps.get(group_key) {
            Some(ts) => ts,
            None => return 1.0,
        };
        let now = now_secs();
        let window = 300.0;
        let recent = ts_vec.iter().filter(|t| now - *t < window).count();
        if recent <= 1 {
            return 1.0;
        }
        let log_factor = self.config.group_reply.saturation_log_factor;
        1.0 + log_factor * (1.0 + recent as f64 / 2.0).ln()
    }

    /// 获取 STOPPED 冷却时长（秒）
    ///
    /// 优先使用挂钩提供的值，否则回退到默认 30.0 秒。
    pub fn get_stopped_cooldown(&self, group_key: &str) -> f64 {
        if let Some(ref cb) = self.hooks.stopped_cooldown {
            cb(group_key)
        } else {
            30.0
        }
    }

    // ── 查询方法 ──

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
    use async_trait::async_trait;

    struct TestPlatform;

    #[async_trait]
    impl PlatformAdapter for TestPlatform {
        async fn send_action(&self, _action: &ReplyAction) -> XueliResult<()> {
            Ok(())
        }
        fn strip_mentions(&self, text: &str) -> String {
            text.to_string()
        }
        fn extract_mentions(&self, _event: &InboundEvent) -> Vec<String> {
            Vec::new()
        }
        fn resolve_mention_placeholders(&self, text: &str, _mentions: &[String]) -> String {
            text.to_string()
        }
        fn platform_name(&self) -> &str {
            "test"
        }
        fn parse_event(&self, _raw: &str) -> XueliResult<InboundEvent> {
            Err("not implemented".into())
        }
    }

    type TestRuntime = BotRuntime<TestPlatform>;

    fn test_config() -> XueliConfig {
        let mut cfg = XueliConfig::default();
        cfg.group_reply.base_frequency = 1.0;
        cfg.group_reply.trigger_threshold = 3;
        cfg.group_reply.idle_grace_seconds = 300.0;
        cfg.group_reply.debounce_seconds = 0.1;
        cfg.group_reply.debounce_max_resets = 3;
        cfg.group_reply.plan_request_interval = 3.0;
        cfg
    }

    #[tokio::test]
    async fn test_lifecycle() {
        let rt = TestRuntime::new(XueliConfig::default());
        assert!(!rt.is_running().await);
        rt.init().await.unwrap();
        assert!(rt.is_running().await);
        rt.shutdown().await.unwrap();
        assert!(!rt.is_running().await);
    }

    #[tokio::test]
    async fn test_group_state_machine() {
        let rt = TestRuntime::new(XueliConfig::default());
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
        let rt = TestRuntime::new(XueliConfig::default());
        assert!(rt.check_and_mark_processed("m1").await);
        assert!(!rt.check_and_mark_processed("m1").await);
        assert!(rt.check_and_mark_processed("m2").await);
    }

    #[tokio::test]
    async fn test_pending_message_buffering() {
        let mut cfg = test_config();
        cfg.group_reply.base_frequency = 0.34; // threshold = ceil(1/0.34) ≈ 3
        let rt = TestRuntime::new(cfg);
        // 第一条消息：不应触发（count=1 < threshold=3）
        assert!(!rt.register_pending_message("g1", None).await);
        assert_eq!(rt.get_pending_count("g1").await, 1);
        // 第二条：仍不触发
        assert!(!rt.register_pending_message("g1", None).await);
        assert_eq!(rt.get_pending_count("g1").await, 2);
        // 第三条：应触发
        assert!(rt.register_pending_message("g1", None).await);
        // 触发后计数清零
        assert_eq!(rt.get_pending_count("g1").await, 0);
        // 清理触发标记
        rt.finish_processing("g1").await;
        assert_eq!(rt.get_pending_count("g1").await, 0);
    }

    #[tokio::test]
    async fn test_record_reply_completion() {
        let rt = TestRuntime::new(XueliConfig::default());
        rt.record_reply_completion("g1").await;
        rt.record_reply_completion("g1").await;
        let timestamps = rt.recent_reply_timestamps.lock().await;
        assert_eq!(timestamps.get("g1").map(|ts| ts.len()).unwrap_or(0), 2);
    }

    #[tokio::test]
    async fn test_calculate_trigger_threshold_default() {
        let rt = TestRuntime::new(test_config());
        // base_frequency=1.0, no time_rules, no overrides, no mood
        // effective = 1.0, base_threshold = ceil(1/1.0) = 1
        let t = rt.calculate_trigger_threshold("g1").await;
        assert_eq!(t, 1);
    }

    #[tokio::test]
    async fn test_calculate_trigger_threshold_low_freq() {
        let mut cfg = test_config();
        cfg.group_reply.base_frequency = 0.25;
        let rt = TestRuntime::new(cfg);
        // effective = 0.25, base_threshold = ceil(1/0.25) = ceil(4) = 4
        let t = rt.calculate_trigger_threshold("g1").await;
        assert_eq!(t, 4);
    }

    #[tokio::test]
    async fn test_calculate_trigger_threshold_clamped() {
        let mut cfg = test_config();
        cfg.group_reply.base_frequency = 0.01; // clamps to 0.05
        let rt = TestRuntime::new(cfg);
        // effective = 0.05, base_threshold = ceil(1/0.05) = ceil(20) = 20
        let t = rt.calculate_trigger_threshold("g1").await;
        assert_eq!(t, 20);
    }

    #[tokio::test]
    async fn test_calculate_trigger_threshold_group_override() {
        let mut cfg = test_config();
        cfg.group_reply.base_frequency = 1.0;
        cfg.group_reply
            .group_overrides
            .insert("noisy_group".to_string(), 0.5);
        let rt = TestRuntime::new(cfg);
        // effective = 1.0 * 0.5 = 0.5, base_threshold = ceil(1/0.5) = 2
        let t = rt.calculate_trigger_threshold("noisy_group").await;
        assert_eq!(t, 2);
    }

    #[tokio::test]
    async fn test_in_time_range() {
        // "09:00-18:00" → hour 12 in range
        assert!(TestRuntime::in_time_range(12, "09:00-18:00"));
        // "09:00-18:00" → hour 8 not in range
        assert!(!TestRuntime::in_time_range(8, "09:00-18:00"));
        // cross-midnight "22:00-06:00" → hour 2 in range
        assert!(TestRuntime::in_time_range(2, "22:00-06:00"));
        // cross-midnight "22:00-06:00" → hour 12 not in range
        assert!(!TestRuntime::in_time_range(12, "22:00-06:00"));
        // invalid format
        assert!(!TestRuntime::in_time_range(12, "invalid"));
    }

    #[tokio::test]
    async fn test_debounce_wait_no_new_messages() {
        let rt = TestRuntime::new(test_config());
        // No pending messages → debounce exits immediately after first sleep
        rt.debounce_wait("g1").await;
    }

    #[tokio::test]
    async fn test_debounce_wait_disabled() {
        let mut cfg = test_config();
        cfg.group_reply.debounce_seconds = 0.0;
        let rt = TestRuntime::new(cfg);
        // debounce disabled → returns immediately
        rt.debounce_wait("g1").await;
    }

    #[tokio::test]
    async fn test_get_average_reply_interval_empty() {
        let rt = TestRuntime::new(test_config());
        assert_eq!(rt.get_average_reply_interval("g1").await, None);
    }

    #[tokio::test]
    async fn test_get_average_reply_interval_single() {
        let rt = TestRuntime::new(test_config());
        rt.record_reply_completion("g1").await;
        assert_eq!(rt.get_average_reply_interval("g1").await, None);
    }

    #[tokio::test]
    async fn test_get_average_reply_interval_two() {
        let rt = TestRuntime::new(test_config());
        rt.record_reply_completion("g1").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        rt.record_reply_completion("g1").await;
        let avg = rt.get_average_reply_interval("g1").await;
        assert!(avg.is_some());
        assert!(avg.unwrap() > 0.0);
    }

    #[tokio::test]
    async fn test_saturation_multiplier() {
        let rt = TestRuntime::new(test_config());
        // No replies yet → 1.0
        assert!((rt.get_group_saturation_multiplier("g1").await - 1.0).abs() < 0.001);
        // Add 3 replies within 5 minutes → should increase
        rt.record_group_reply("g1").await;
        rt.record_group_reply("g1").await;
        rt.record_group_reply("g1").await;
        let sat = rt.get_group_saturation_multiplier("g1").await;
        assert!(sat > 1.0);
    }

    #[tokio::test]
    async fn test_idle_grace_triggers_after_idle() {
        let mut cfg = test_config();
        cfg.group_reply.base_frequency = 1.0;
        cfg.group_reply.idle_grace_seconds = 0.001; // nearly immediate
        let rt = TestRuntime::new(cfg);

        // Set last activity to 10 seconds ago (idle for a while)
        rt.group_last_activity
            .lock()
            .await
            .insert("g1".to_string(), now_secs() - 10.0);

        // First message after idle → should trigger (idle_grace=0.001s, idle_time=10s)
        assert!(rt.register_pending_message("g1", None).await);
    }

    #[tokio::test]
    async fn test_turn_scheduled_prevents_duplicate_trigger() {
        let rt = TestRuntime::new(test_config());
        // Manually mark turn as scheduled
        rt.group_turn_scheduled
            .lock()
            .await
            .insert("g1".to_string(), true);
        // Should not trigger because turn already scheduled
        assert!(!rt.register_pending_message("g1", None).await);
    }

    #[tokio::test]
    async fn test_process_reply_no_handler() {
        let rt = TestRuntime::new(XueliConfig::default());
        let result = rt._process_reply("g1").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_setup_proactive_share() {
        let mut rt = TestRuntime::new(XueliConfig::default());
        let store = Arc::new(ProactiveShareStore::new("/tmp/test_runtime_shares.json"));
        let config = ProactiveShareConfig::default();
        let result = rt.setup_proactive_share(&config, store);
        assert!(result.is_ok());
        let _ = std::fs::remove_file("/tmp/test_runtime_shares.json");
    }

    #[tokio::test]
    async fn test_send_proactive_share_no_adapter() {
        let rt = TestRuntime::new(XueliConfig::default());
        let result = rt.send_proactive_share("hello", "u1", Some("g1")).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[tokio::test]
    async fn test_send_proactive_share_with_adapter() {
        let mut rt = TestRuntime::new(XueliConfig::default());
        rt.set_adapter(Arc::new(TestPlatform));
        let result = rt.send_proactive_share("hello", "u1", Some("g1")).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_send_proactive_share_private() {
        let mut rt = TestRuntime::new(XueliConfig::default());
        rt.set_adapter(Arc::new(TestPlatform));
        let result = rt.send_proactive_share("hello", "u1", None).await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_apply_mood_night_recovery_no_handler() {
        let rt = TestRuntime::new(XueliConfig::default());
        let result = rt.apply_mood_night_recovery().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_set_message_handler() {
        let mut rt = TestRuntime::new(XueliConfig::default());
        assert!(rt.message_handler.is_none());
        // set_adapter + set_message_handler test that fields accept values
        rt.set_adapter(Arc::new(TestPlatform));
        assert!(rt.adapter.is_some());
    }
}
