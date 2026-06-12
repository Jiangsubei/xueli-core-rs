use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// 运行时指标 — 轻量级内存指标门面，线程安全
pub struct RuntimeMetrics {
    // ── 时间追踪 ──────────────────────────────────────────────
    started_at: Instant,
    last_error_at: Mutex<Option<String>>,

    // ── 状态标记 ──────────────────────────────────────────────
    ready: AtomicU64,
    connected: AtomicU64,

    // ── 消息统计 ──────────────────────────────────────────────
    total_messages_received: AtomicU64,
    total_replies_sent: AtomicU64,
    reply_parts_sent: AtomicU64,
    message_errors: AtomicU64,
    total_ignored: AtomicU64,

    // ── AI 统计 ───────────────────────────────────────────────
    total_ai_calls: AtomicU64,
    total_tokens_used: AtomicU64,
    avg_response_latency_ms: std::sync::Mutex<f64>,

    // ── 命令统计 ──────────────────────────────────────────────
    command_hits: AtomicU64,
    command_hits_by_name: Mutex<HashMap<String, u64>>,

    // ── 规划器统计 ────────────────────────────────────────────
    planner_reply: AtomicU64,
    planner_wait: AtomicU64,
    planner_ignore: AtomicU64,

    // ── 视觉统计 ──────────────────────────────────────────────
    vision_requests: AtomicU64,
    vision_images_processed: AtomicU64,
    vision_failures: AtomicU64,
    vision_reused_from_plan: AtomicU64,

    // ── 表情统计 ──────────────────────────────────────────────
    emoji_detected: AtomicU64,
    emoji_classified: AtomicU64,
    emoji_classification_failures: AtomicU64,
    emoji_reply_decisions: AtomicU64,
    emoji_reply_sent: AtomicU64,
    emoji_reply_skipped: AtomicU64,
    emoji_reply_no_candidate: AtomicU64,
    emoji_total: AtomicU64,
    emoji_pending_classification: AtomicU64,
    emoji_disabled: AtomicU64,
    emoji_active_classifiers: AtomicU64,

    // ── 记忆统计 ──────────────────────────────────────────────
    memory_reads: AtomicU64,
    memory_writes: AtomicU64,
    memory_shared_reads: AtomicU64,
    memory_scene_rule_hits: AtomicU64,
    memory_access_denied: AtomicU64,
    memory_migrations: AtomicU64,
    memory_compactions: AtomicU64,

    // ── 系统统计 ──────────────────────────────────────────────
    active_message_tasks: AtomicU64,
    active_session_workers: AtomicU64,
    active_model_workers: AtomicU64,
    pending_model_jobs: AtomicU64,
    active_conversations: AtomicU64,
    background_tasks: AtomicU64,
    wait_inflight: AtomicU64,
    wait_queue_drop: AtomicU64,

    // ── 信号统计 ──────────────────────────────────────────────
    signal_l1_stale_invalidation: AtomicU64,

    // ── 错误统计 ──────────────────────────────────────────────
    error_count: AtomicU64,
}

impl Default for RuntimeMetrics {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            last_error_at: Mutex::new(None),
            ready: AtomicU64::new(0),
            connected: AtomicU64::new(0),
            total_messages_received: AtomicU64::new(0),
            total_replies_sent: AtomicU64::new(0),
            reply_parts_sent: AtomicU64::new(0),
            message_errors: AtomicU64::new(0),
            total_ignored: AtomicU64::new(0),
            total_ai_calls: AtomicU64::new(0),
            total_tokens_used: AtomicU64::new(0),
            avg_response_latency_ms: std::sync::Mutex::new(0.0),
            command_hits: AtomicU64::new(0),
            command_hits_by_name: Mutex::new(HashMap::new()),
            planner_reply: AtomicU64::new(0),
            planner_wait: AtomicU64::new(0),
            planner_ignore: AtomicU64::new(0),
            vision_requests: AtomicU64::new(0),
            vision_images_processed: AtomicU64::new(0),
            vision_failures: AtomicU64::new(0),
            vision_reused_from_plan: AtomicU64::new(0),
            emoji_detected: AtomicU64::new(0),
            emoji_classified: AtomicU64::new(0),
            emoji_classification_failures: AtomicU64::new(0),
            emoji_reply_decisions: AtomicU64::new(0),
            emoji_reply_sent: AtomicU64::new(0),
            emoji_reply_skipped: AtomicU64::new(0),
            emoji_reply_no_candidate: AtomicU64::new(0),
            emoji_total: AtomicU64::new(0),
            emoji_pending_classification: AtomicU64::new(0),
            emoji_disabled: AtomicU64::new(0),
            emoji_active_classifiers: AtomicU64::new(0),
            memory_reads: AtomicU64::new(0),
            memory_writes: AtomicU64::new(0),
            memory_shared_reads: AtomicU64::new(0),
            memory_scene_rule_hits: AtomicU64::new(0),
            memory_access_denied: AtomicU64::new(0),
            memory_migrations: AtomicU64::new(0),
            memory_compactions: AtomicU64::new(0),
            active_message_tasks: AtomicU64::new(0),
            active_session_workers: AtomicU64::new(0),
            active_model_workers: AtomicU64::new(0),
            pending_model_jobs: AtomicU64::new(0),
            active_conversations: AtomicU64::new(0),
            background_tasks: AtomicU64::new(0),
            wait_inflight: AtomicU64::new(0),
            wait_queue_drop: AtomicU64::new(0),
            signal_l1_stale_invalidation: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
        }
    }
}

impl RuntimeMetrics {
    // ── 基础状态 ──────────────────────────────────────────────

    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready as u64, Ordering::Relaxed);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed) != 0
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected as u64, Ordering::Relaxed);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed) != 0
    }

    // ── 消息统计 ──────────────────────────────────────────────

    pub fn record_message_received(&self) {
        self.total_messages_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_message_received_count(&self, count: u64) {
        self.total_messages_received
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_reply_sent(&self, latency_ms: f64) {
        self.total_replies_sent.fetch_add(1, Ordering::Relaxed);
        self.reply_parts_sent.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut avg) = self.avg_response_latency_ms.lock() {
            let alpha = 0.1;
            *avg = alpha * latency_ms + (1.0 - alpha) * *avg;
        }
    }

    pub fn record_reply_parts_sent(&self, parts: u64) {
        self.reply_parts_sent.fetch_add(parts, Ordering::Relaxed);
    }

    pub fn record_ignored(&self) {
        self.total_ignored.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut last) = self.last_error_at.lock() {
            *last = Some(chrono::Local::now().to_rfc3339());
        }
    }

    pub fn record_message_error(&self, count: u64) {
        self.message_errors.fetch_add(count, Ordering::Relaxed);
    }

    // ── AI 统计 ───────────────────────────────────────────────

    pub fn record_ai_call(&self, tokens: u64) {
        self.total_ai_calls.fetch_add(1, Ordering::Relaxed);
        self.total_tokens_used.fetch_add(tokens, Ordering::Relaxed);
    }

    // ── 命令统计 ──────────────────────────────────────────────

    pub fn record_command_hit(&self, name: &str) {
        self.command_hits.fetch_add(1, Ordering::Relaxed);
        let normalized = if name.trim().is_empty() {
            "unknown"
        } else {
            name
        }
        .trim()
        .to_lowercase();
        if let Ok(mut map) = self.command_hits_by_name.lock() {
            *map.entry(normalized).or_default() += 1;
        }
    }

    // ── 规划器统计 ────────────────────────────────────────────

    pub fn record_planner_action(&self, action: &str) {
        match action.trim().to_lowercase().as_str() {
            "reply" => {
                self.planner_reply.fetch_add(1, Ordering::Relaxed);
            }
            "wait" => {
                self.planner_wait.fetch_add(1, Ordering::Relaxed);
            }
            "ignore" => {
                self.planner_ignore.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    // ── 视觉统计 ──────────────────────────────────────────────

    pub fn record_vision_request(
        &self,
        image_count: u64,
        failure_count: u64,
        reused_from_plan: bool,
    ) {
        if reused_from_plan {
            self.vision_reused_from_plan.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.vision_requests.fetch_add(1, Ordering::Relaxed);
        self.vision_images_processed
            .fetch_add(image_count, Ordering::Relaxed);
        self.vision_failures
            .fetch_add(failure_count, Ordering::Relaxed);
    }

    // ── 表情统计 ──────────────────────────────────────────────

    pub fn record_emoji_detection(&self, count: u64) {
        self.emoji_detected.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_emoji_classification(&self, count: u64) {
        self.emoji_classified.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_emoji_classification_failure(&self, count: u64) {
        self.emoji_classification_failures
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_emoji_reply_decision(&self, count: u64) {
        self.emoji_reply_decisions
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_emoji_reply_sent(&self, count: u64) {
        self.emoji_reply_sent.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_emoji_reply_skipped(&self, count: u64) {
        self.emoji_reply_skipped.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_emoji_reply_no_candidate(&self, count: u64) {
        self.emoji_reply_no_candidate
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_emoji_total(&self, count: u64) {
        self.emoji_total.fetch_add(count, Ordering::Relaxed);
    }

    pub fn set_emoji_pending_classification(&self, count: u64) {
        self.emoji_pending_classification
            .store(count, Ordering::Relaxed);
    }

    pub fn set_emoji_disabled(&self, count: u64) {
        self.emoji_disabled.store(count, Ordering::Relaxed);
    }

    pub fn set_emoji_active_classifiers(&self, count: u64) {
        self.emoji_active_classifiers
            .store(count, Ordering::Relaxed);
    }

    // ── 记忆统计 ──────────────────────────────────────────────

    pub fn record_memory_read(&self, count: u64) {
        self.memory_reads.fetch_add(count, Ordering::Relaxed);
    }

    pub fn inc_memory_read(&self, shared: bool) {
        self.memory_reads.fetch_add(1, Ordering::Relaxed);
        if shared {
            self.memory_shared_reads.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn inc_memory_scene_rule_hits(&self, count: u64) {
        self.memory_scene_rule_hits
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_memory_write(&self, count: u64) {
        self.memory_writes.fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_memory_access_denied(&self, count: u64) {
        self.memory_access_denied
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn inc_memory_migration(&self, count: u64) {
        self.memory_migrations.fetch_add(count, Ordering::Relaxed);
    }

    pub fn inc_memory_compaction(&self, count: u64) {
        self.memory_compactions.fetch_add(count, Ordering::Relaxed);
    }

    // ── 系统统计 ──────────────────────────────────────────────

    pub fn set_active_message_tasks(&self, count: u64) {
        self.active_message_tasks.store(count, Ordering::Relaxed);
    }

    pub fn set_active_session_workers(&self, count: u64) {
        self.active_session_workers.store(count, Ordering::Relaxed);
    }

    pub fn set_active_model_workers(&self, count: u64) {
        self.active_model_workers.store(count, Ordering::Relaxed);
    }

    pub fn set_pending_model_jobs(&self, count: u64) {
        self.pending_model_jobs.store(count, Ordering::Relaxed);
    }

    pub fn set_active_conversations(&self, count: u64) {
        self.active_conversations.store(count, Ordering::Relaxed);
    }

    pub fn set_background_tasks(&self, count: u64) {
        self.background_tasks.store(count, Ordering::Relaxed);
    }

    pub fn inc_wait_queue_drop(&self, count: u64) {
        self.wait_queue_drop.fetch_add(count, Ordering::Relaxed);
    }

    // ── 信号统计 ──────────────────────────────────────────────

    pub fn record_signal_l1_stale_invalidation(&self, count: u64) {
        self.signal_l1_stale_invalidation
            .fetch_add(count, Ordering::Relaxed);
    }

    // ── 快照 ──────────────────────────────────────────────────

    /// 返回所有指标的快照（含 uptime_seconds 和 last_error_at）
    pub fn snapshot(&self) -> HashMap<String, serde_json::Value> {
        let mut map = HashMap::new();
        map.insert(
            "ready".to_string(),
            serde_json::Value::Bool(self.is_ready()),
        );
        map.insert(
            "connected".to_string(),
            serde_json::Value::Bool(self.is_connected()),
        );
        map.insert(
            "uptime_seconds".to_string(),
            serde_json::Value::Number(
                serde_json::Number::from_f64(self.started_at.elapsed().as_secs_f64().round())
                    .unwrap_or(serde_json::Number::from(0)),
            ),
        );
        map.insert(
            "last_error_at".to_string(),
            serde_json::Value::String(
                self.last_error_at
                    .lock()
                    .ok()
                    .and_then(|v| v.clone())
                    .unwrap_or_default(),
            ),
        );

        let u64_fields = [
            (
                "total_messages_received",
                self.total_messages_received.load(Ordering::Relaxed),
            ),
            (
                "total_replies_sent",
                self.total_replies_sent.load(Ordering::Relaxed),
            ),
            (
                "reply_parts_sent",
                self.reply_parts_sent.load(Ordering::Relaxed),
            ),
            (
                "message_errors",
                self.message_errors.load(Ordering::Relaxed),
            ),
            ("total_ignored", self.total_ignored.load(Ordering::Relaxed)),
            (
                "total_ai_calls",
                self.total_ai_calls.load(Ordering::Relaxed),
            ),
            (
                "total_tokens_used",
                self.total_tokens_used.load(Ordering::Relaxed),
            ),
            ("command_hits", self.command_hits.load(Ordering::Relaxed)),
            ("planner_reply", self.planner_reply.load(Ordering::Relaxed)),
            ("planner_wait", self.planner_wait.load(Ordering::Relaxed)),
            (
                "planner_ignore",
                self.planner_ignore.load(Ordering::Relaxed),
            ),
            (
                "vision_requests",
                self.vision_requests.load(Ordering::Relaxed),
            ),
            (
                "vision_images_processed",
                self.vision_images_processed.load(Ordering::Relaxed),
            ),
            (
                "vision_failures",
                self.vision_failures.load(Ordering::Relaxed),
            ),
            (
                "vision_reused_from_plan",
                self.vision_reused_from_plan.load(Ordering::Relaxed),
            ),
            (
                "emoji_detected",
                self.emoji_detected.load(Ordering::Relaxed),
            ),
            (
                "emoji_classified",
                self.emoji_classified.load(Ordering::Relaxed),
            ),
            (
                "emoji_classification_failures",
                self.emoji_classification_failures.load(Ordering::Relaxed),
            ),
            (
                "emoji_reply_decisions",
                self.emoji_reply_decisions.load(Ordering::Relaxed),
            ),
            (
                "emoji_reply_sent",
                self.emoji_reply_sent.load(Ordering::Relaxed),
            ),
            (
                "emoji_reply_skipped",
                self.emoji_reply_skipped.load(Ordering::Relaxed),
            ),
            (
                "emoji_reply_no_candidate",
                self.emoji_reply_no_candidate.load(Ordering::Relaxed),
            ),
            ("emoji_total", self.emoji_total.load(Ordering::Relaxed)),
            (
                "emoji_pending_classification",
                self.emoji_pending_classification.load(Ordering::Relaxed),
            ),
            (
                "emoji_disabled",
                self.emoji_disabled.load(Ordering::Relaxed),
            ),
            (
                "emoji_active_classifiers",
                self.emoji_active_classifiers.load(Ordering::Relaxed),
            ),
            ("memory_reads", self.memory_reads.load(Ordering::Relaxed)),
            ("memory_writes", self.memory_writes.load(Ordering::Relaxed)),
            (
                "memory_shared_reads",
                self.memory_shared_reads.load(Ordering::Relaxed),
            ),
            (
                "memory_scene_rule_hits",
                self.memory_scene_rule_hits.load(Ordering::Relaxed),
            ),
            (
                "memory_access_denied",
                self.memory_access_denied.load(Ordering::Relaxed),
            ),
            (
                "memory_migrations",
                self.memory_migrations.load(Ordering::Relaxed),
            ),
            (
                "memory_compactions",
                self.memory_compactions.load(Ordering::Relaxed),
            ),
            (
                "active_message_tasks",
                self.active_message_tasks.load(Ordering::Relaxed),
            ),
            (
                "active_session_workers",
                self.active_session_workers.load(Ordering::Relaxed),
            ),
            (
                "active_model_workers",
                self.active_model_workers.load(Ordering::Relaxed),
            ),
            (
                "pending_model_jobs",
                self.pending_model_jobs.load(Ordering::Relaxed),
            ),
            (
                "active_conversations",
                self.active_conversations.load(Ordering::Relaxed),
            ),
            (
                "background_tasks",
                self.background_tasks.load(Ordering::Relaxed),
            ),
            ("wait_inflight", self.wait_inflight.load(Ordering::Relaxed)),
            (
                "wait_queue_drop",
                self.wait_queue_drop.load(Ordering::Relaxed),
            ),
            (
                "signal_l1_stale_invalidation",
                self.signal_l1_stale_invalidation.load(Ordering::Relaxed),
            ),
            ("error_count", self.error_count.load(Ordering::Relaxed)),
        ];
        for (key, value) in u64_fields {
            map.insert(key.to_string(), serde_json::Value::Number(value.into()));
        }

        if let Ok(avg) = self.avg_response_latency_ms.lock() {
            map.insert(
                "avg_response_latency_ms".to_string(),
                serde_json::Value::Number(
                    serde_json::Number::from_f64(*avg).unwrap_or(serde_json::Number::from(0)),
                ),
            );
        }

        if let Ok(cmd_map) = self.command_hits_by_name.lock() {
            let mut cmd_obj = serde_json::Map::new();
            for (name, count) in cmd_map.iter() {
                cmd_obj.insert(name.clone(), serde_json::Value::Number((*count).into()));
            }
            map.insert(
                "command_hits_by_name".to_string(),
                serde_json::Value::Object(cmd_obj),
            );
        }

        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_metrics() {
        let metrics = RuntimeMetrics::default();
        assert!(!metrics.is_ready());
        assert!(!metrics.is_connected());
        assert_eq!(metrics.total_messages_received.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_record_message_received() {
        let metrics = RuntimeMetrics::default();
        metrics.record_message_received();
        metrics.record_message_received_count(5);
        assert_eq!(metrics.total_messages_received.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn test_record_reply_sent() {
        let metrics = RuntimeMetrics::default();
        metrics.record_reply_sent(100.0);
        assert_eq!(metrics.total_replies_sent.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.reply_parts_sent.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_record_error_updates_last_error_at() {
        let metrics = RuntimeMetrics::default();
        metrics.record_error();
        let last = metrics.last_error_at.lock().unwrap();
        assert!(last.is_some());
    }

    #[test]
    fn test_snapshot_contains_uptime() {
        let metrics = RuntimeMetrics::default();
        let snap = metrics.snapshot();
        assert!(snap.contains_key("uptime_seconds"));
        assert!(snap.contains_key("last_error_at"));
        assert!(snap.contains_key("command_hits_by_name"));
    }

    #[test]
    fn test_inc_memory_read_shared() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_memory_read(true);
        assert_eq!(metrics.memory_reads.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.memory_shared_reads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_inc_memory_read_not_shared() {
        let metrics = RuntimeMetrics::default();
        metrics.inc_memory_read(false);
        assert_eq!(metrics.memory_reads.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.memory_shared_reads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_new_counters_in_snapshot() {
        let metrics = RuntimeMetrics::default();
        let snap = metrics.snapshot();
        assert!(snap.contains_key("active_session_workers"));
        assert!(snap.contains_key("active_model_workers"));
        assert!(snap.contains_key("pending_model_jobs"));
        assert!(snap.contains_key("wait_inflight"));
        assert!(snap.contains_key("wait_queue_drop"));
        assert!(snap.contains_key("memory_shared_reads"));
        assert!(snap.contains_key("memory_scene_rule_hits"));
        assert!(snap.contains_key("memory_migrations"));
        assert!(snap.contains_key("memory_compactions"));
        assert!(snap.contains_key("emoji_pending_classification"));
        assert!(snap.contains_key("emoji_disabled"));
        assert!(snap.contains_key("emoji_active_classifiers"));
    }
}
