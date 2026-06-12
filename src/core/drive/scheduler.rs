//! DriveScheduler — 独立调度器：事件监听、轮次计数、反思触发、定时任务。
//!
//! 职责：
//!   - 注册为 MessageHandler 事件监听器
//!   - 管理每条消息的事件增量
//!   - 轮次计数与反思触发判断
//!   - 定时衰减 tick
//!   - 定时反思
//!   - 夜间恢复调度

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::info;

use crate::traits::ai_client::AIClient;

use super::engine::DriveEngine;
use super::models::{DriveContext, EventLogEntry};
use super::reflection::{DriveReflection, DynTemplateLoader};
use super::store::DriveStore;

/// 内驱力系统独立调度器。
pub struct DriveScheduler {
    data_dir: PathBuf,
    ai_client: Option<Arc<dyn AIClient>>,
    template_loader: Option<Arc<dyn DynTemplateLoader>>,
    reflection_trigger_rounds: usize,
    reflection_trigger_interval_secs: u64,
    event_decay_tick_interval_secs: u64,
    max_rule_weight_adjustment: f64,
    enabled: bool,

    /// 每个作用域的引擎实例
    engines: Arc<Mutex<HashMap<String, DriveEngine>>>,
    /// 每个作用域的轮次计数
    round_counts: Arc<Mutex<HashMap<String, usize>>>,
    /// 每个作用域的上次反思时间（epoch 秒）
    last_reflection_at: Arc<Mutex<HashMap<String, u64>>>,
    /// 近期事件日志
    event_log: Arc<Mutex<HashMap<String, Vec<EventLogEntry>>>>,

    /// 后台任务句柄
    decay_task: Option<JoinHandle<()>>,
    reflection_task: Option<JoinHandle<()>>,
    shutdown: Arc<tokio::sync::Notify>,
}

impl DriveScheduler {
    pub fn new(
        data_dir: impl Into<PathBuf>,
        ai_client: Option<Arc<dyn AIClient>>,
        template_loader: Option<Arc<dyn DynTemplateLoader>>,
        reflection_trigger_rounds: usize,
        reflection_trigger_interval_secs: u64,
        event_decay_tick_interval_secs: u64,
        max_rule_weight_adjustment: f64,
        enabled: bool,
    ) -> Self {
        Self {
            data_dir: data_dir.into(),
            ai_client,
            template_loader,
            reflection_trigger_rounds: reflection_trigger_rounds.max(1),
            reflection_trigger_interval_secs: reflection_trigger_interval_secs.max(60),
            event_decay_tick_interval_secs: event_decay_tick_interval_secs.max(1),
            max_rule_weight_adjustment,
            enabled,
            engines: Arc::new(Mutex::new(HashMap::new())),
            round_counts: Arc::new(Mutex::new(HashMap::new())),
            last_reflection_at: Arc::new(Mutex::new(HashMap::new())),
            event_log: Arc::new(Mutex::new(HashMap::new())),
            decay_task: None,
            reflection_task: None,
            shutdown: Arc::new(tokio::sync::Notify::new()),
        }
    }

    // ─── 生命周期 ───────────────────────────────────────

    /// 启动调度器后台任务。
    pub async fn start(&mut self) {
        if !self.enabled {
            info!("[DriveScheduler] 内驱力系统未启用");
            return;
        }

        let decay_interval = self.event_decay_tick_interval_secs;
        let engines = self.engines.clone();

        // 衰减循环
        let eng_clone = engines.clone();
        let shut_clone = self.shutdown.clone();
        let decay_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shut_clone.notified() => break,
                    _ = tokio::time::sleep(Duration::from_secs(decay_interval)) => {
                        let mut engines = eng_clone.lock().await;
                        for engine in engines.values_mut() {
                            engine.decay_tick().await;
                        }
                    }
                }
            }
        });

        // 反思循环 — 定期检查各作用域是否需要反思
        let reflection_interval = self.reflection_trigger_interval_secs;
        let eng_clone2 = engines.clone();
        let shut_clone2 = self.shutdown.clone();
        let round_counts = self.round_counts.clone();
        let last_reflection_at = self.last_reflection_at.clone();
        let event_log = self.event_log.clone();
        let ai_client = self.ai_client.clone();
        let template_loader = self.template_loader.clone();
        let max_adj = self.max_rule_weight_adjustment;
        let trigger_rounds = self.reflection_trigger_rounds;
        let trigger_interval = self.reflection_trigger_interval_secs;

        let reflection_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shut_clone2.notified() => break,
                    _ = tokio::time::sleep(Duration::from_secs(reflection_interval)) => {
                        let scope_keys: Vec<String> = {
                            let engines = eng_clone2.lock().await;
                            engines.keys().cloned().collect()
                        };
                        for scope_key in scope_keys {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();

                            // 检查间隔限制
                            {
                                let last = last_reflection_at.lock().await;
                                if let Some(&last_time) = last.get(&scope_key) {
                                    if now - last_time < trigger_interval {
                                        continue;
                                    }
                                }
                            }

                            // 检查轮次
                            {
                                let counts = round_counts.lock().await;
                                if let Some(&count) = counts.get(&scope_key) {
                                    if count < trigger_rounds {
                                        continue;
                                    }
                                }
                            }

                            // 执行反思
                            if let (Some(ref ai), Some(ref tpl)) = (&ai_client, &template_loader) {
                                let (snapshot, log, round_count) = {
                                    let engines = eng_clone2.lock().await;
                                    let engine = match engines.get(&scope_key) {
                                        Some(e) => e,
                                        None => continue,
                                    };
                                    let ctx = engine.get_drive_context("");
                                    let snap = super::models::DriveSnapshot {
                                        affective: super::models::AffectiveState {
                                            pad: ctx.affective.clone(),
                                            updated_at: String::new(),
                                        },
                                        motivational: engine.get_motivational_state(),
                                        relational: Default::default(),
                                        event_rules: engine.get_event_rules().clone(),
                                        scope_key: scope_key.clone(),
                                        version: 1,
                                        created_at: String::new(),
                                        updated_at: String::new(),
                                    };
                                    let log = event_log.lock().await.get(&scope_key).cloned().unwrap_or_default();
                                    let count = round_counts.lock().await.get(&scope_key).copied().unwrap_or(0);
                                    (snap, log, count)
                                };

                                let reflection = DriveReflection::new(
                                    Some(ai.clone()),
                                    Some(tpl.clone()),
                                    max_adj,
                                );

                                if let Some(result) = reflection.run_reflection(&snapshot, &log, round_count).await {
                                    let mut engines = eng_clone2.lock().await;
                                    if let Some(engine) = engines.get_mut(&scope_key) {
                                        engine.apply_reflection_result(&result).await;
                                    }
                                    event_log.lock().await.remove(&scope_key);
                                    last_reflection_at.lock().await.insert(scope_key.clone(), now);
                                    round_counts.lock().await.insert(scope_key.clone(), 0);
                                    info!("[DriveScheduler] 反思完成: scope={} confidence={:.2}", scope_key, result.confidence);
                                }
                            }
                        }
                    }
                }
            }
        });

        self.decay_task = Some(decay_handle);
        self.reflection_task = Some(reflection_handle);
        info!("[DriveScheduler] 已启动");
    }

    /// 停止调度器。
    pub async fn stop(&mut self) {
        self.shutdown.notify_waiters();

        if let Some(handle) = self.decay_task.take() {
            handle.abort();
        }
        if let Some(handle) = self.reflection_task.take() {
            handle.abort();
        }

        info!("[DriveScheduler] 已停止");
    }

    // ─── 事件监听接口 ───────────────────────────────────

    /// 钩子 A：Phase 1 前触发，用于事件增量。
    pub async fn on_inbound_event(&self, scope_key: &str, event_patterns: &[String]) {
        if !self.enabled || scope_key.is_empty() || event_patterns.is_empty() {
            return;
        }

        let mut engines = self.engines.lock().await;
        let engine = engines.entry(scope_key.to_string()).or_insert_with(|| {
            DriveEngine::new(DriveStore::new(&self.data_dir), scope_key, self.enabled)
        });
        engine.apply_event_deltas(event_patterns).await;

        // 记录事件日志
        let mut event_log = self.event_log.lock().await;
        let log = event_log
            .entry(scope_key.to_string())
            .or_insert_with(Vec::new);
        let now = chrono::Local::now().format("%H:%M:%S").to_string();
        for pattern in event_patterns {
            log.push(EventLogEntry {
                pattern: pattern.clone(),
                timestamp: now.clone(),
            });
        }
        // 保留最近 100 条
        if log.len() > 100 {
            let drain_count = log.len() - 100;
            log.drain(..drain_count);
        }
    }

    /// 钩子 B：Phase 5 后触发，用于轮次计数与反思判断。
    pub async fn on_reply_completed(&self, scope_key: &str) {
        if !self.enabled || scope_key.is_empty() {
            return;
        }

        // 轮次计数递增
        let mut round_counts = self.round_counts.lock().await;
        let count = round_counts.entry(scope_key.to_string()).or_insert(0);
        *count += 1;
    }

    // ─── 定时任务 ───────────────────────────────────────

    /// 对所有引擎执行夜间恢复。
    pub async fn apply_night_recovery(&self) {
        let mut engines = self.engines.lock().await;
        for engine in engines.values_mut() {
            engine.night_recovery().await;
        }
    }

    /// 从上下文构建阶段注入事件增量。
    pub async fn apply_context_events(&self, scope_key: &str, event_patterns: &[String]) {
        if !self.enabled || event_patterns.is_empty() {
            return;
        }
        let mut engines = self.engines.lock().await;
        let engine = engines.entry(scope_key.to_string()).or_insert_with(|| {
            DriveEngine::new(DriveStore::new(&self.data_dir), scope_key, self.enabled)
        });
        engine.apply_event_deltas(event_patterns).await;
    }

    // ─── 公共查询接口 ───────────────────────────────────

    /// 获取指定作用域的内驱力上下文。
    pub async fn get_drive_context(&self, scope_key: &str, user_id: &str) -> Option<DriveContext> {
        let engines = self.engines.lock().await;
        engines.get(scope_key).map(|e| e.get_drive_context(user_id))
    }

    /// 获取指定作用域引擎的谨慎度指导。
    pub async fn get_caution_guidance(&self, scope_key: &str) -> Vec<String> {
        let engines = self.engines.lock().await;
        engines
            .get(scope_key)
            .map(|e| e.get_caution_guidance())
            .unwrap_or_default()
    }

    /// 清空指定作用域引擎的指导。
    pub async fn clear_guidance(&self, scope_key: &str) {
        let mut engines = self.engines.lock().await;
        if let Some(engine) = engines.get_mut(scope_key) {
            engine.clear_guidance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scheduler_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let mut scheduler = DriveScheduler::new(
            dir.path().to_path_buf(),
            None,
            None,
            5,
            3600,
            10,
            0.3,
            false,
        );
        scheduler.start().await;
        scheduler.stop().await;
    }

    #[tokio::test]
    async fn test_on_inbound_event() {
        let dir = tempfile::tempdir().unwrap();
        let scheduler =
            DriveScheduler::new(dir.path().to_path_buf(), None, None, 5, 3600, 10, 0.3, true);
        let patterns = vec!["negative_feedback".to_string()];
        scheduler.on_inbound_event("test_scope", &patterns).await;
        let log = scheduler.event_log.lock().await;
        let entries = log.get("test_scope").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].pattern, "negative_feedback");
    }

    #[tokio::test]
    async fn test_on_reply_completed_count() {
        let dir = tempfile::tempdir().unwrap();
        let scheduler =
            DriveScheduler::new(dir.path().to_path_buf(), None, None, 5, 3600, 10, 0.3, true);
        scheduler.on_reply_completed("test_scope").await;
        let counts = scheduler.round_counts.lock().await;
        assert_eq!(counts.get("test_scope").copied().unwrap_or(0), 1);
    }
}
