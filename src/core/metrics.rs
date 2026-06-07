use std::collections::HashMap;

/// 运行时指标 — 轻量级内存指标门面
#[derive(Debug, Clone)]
pub struct RuntimeMetrics {
    // ── 状态标记 ──────────────────────────────────────────────
    pub ready: bool,
    pub connected: bool,

    // ── 原字段（保持兼容）─────────────────────────────────────
    pub total_messages_received: u64,
    pub total_replies_sent: u64,
    pub total_ignored: u64,
    pub error_count: u64,
    pub avg_response_latency_ms: f64,
    pub total_ai_calls: u64,
    pub total_tokens_used: u64,

    // ── 消息统计 — 新增 ──────────────────────────────────────
    pub reply_parts_sent: u64,
    pub message_errors: u64,

    // ── 命令统计 — 新增 ──────────────────────────────────────
    pub command_hits: u64,
    pub command_hits_by_name: HashMap<String, u64>,

    // ── 规划器统计 — 新增 ─────────────────────────────────────
    pub planner_reply: u64,
    pub planner_wait: u64,
    pub planner_ignore: u64,

    // ── 视觉统计 — 新增 ──────────────────────────────────────
    pub vision_requests: u64,
    pub vision_images_processed: u64,
    pub vision_failures: u64,
    pub vision_reused_from_plan: u64,

    // ── 表情统计 — 新增 ──────────────────────────────────────
    pub emoji_detected: u64,
    pub emoji_classified: u64,
    pub emoji_classification_failures: u64,
    pub emoji_reply_decisions: u64,
    pub emoji_reply_sent: u64,
    pub emoji_reply_skipped: u64,
    pub emoji_reply_no_candidate: u64,
    pub emoji_total: u64,

    // ── 记忆统计 — 新增 ──────────────────────────────────────
    pub memory_reads: u64,
    pub memory_writes: u64,
    pub memory_access_denied: u64,

    // ── 系统统计 — 新增 ──────────────────────────────────────
    pub active_message_tasks: u64,
    pub active_conversations: u64,
    pub background_tasks: u64,

    // ── 信号统计 — 新增 ──────────────────────────────────────
    pub signal_l1_stale_invalidation: u64,
}

impl Default for RuntimeMetrics {
    fn default() -> Self {
        Self {
            ready: false,
            connected: false,
            total_messages_received: 0,
            total_replies_sent: 0,
            total_ignored: 0,
            error_count: 0,
            avg_response_latency_ms: 0.0,
            total_ai_calls: 0,
            total_tokens_used: 0,
            reply_parts_sent: 0,
            message_errors: 0,
            command_hits: 0,
            command_hits_by_name: HashMap::new(),
            planner_reply: 0,
            planner_wait: 0,
            planner_ignore: 0,
            vision_requests: 0,
            vision_images_processed: 0,
            vision_failures: 0,
            vision_reused_from_plan: 0,
            emoji_detected: 0,
            emoji_classified: 0,
            emoji_classification_failures: 0,
            emoji_reply_decisions: 0,
            emoji_reply_sent: 0,
            emoji_reply_skipped: 0,
            emoji_reply_no_candidate: 0,
            emoji_total: 0,
            memory_reads: 0,
            memory_writes: 0,
            memory_access_denied: 0,
            active_message_tasks: 0,
            active_conversations: 0,
            background_tasks: 0,
            signal_l1_stale_invalidation: 0,
        }
    }
}

impl RuntimeMetrics {
    // ── 基础状态 ──────────────────────────────────────────────

    pub fn set_ready(&mut self, ready: bool) {
        self.ready = ready;
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    // ── 消息统计（保留旧签名兼容）─────────────────────────────

    pub fn record_message_received(&mut self) {
        self.total_messages_received += 1;
    }

    pub fn record_reply_sent(&mut self, latency_ms: f64) {
        self.total_replies_sent += 1;
        self.reply_parts_sent += 1;
        let alpha = 0.1;
        self.avg_response_latency_ms =
            alpha * latency_ms + (1.0 - alpha) * self.avg_response_latency_ms;
    }

    pub fn record_ignored(&mut self) {
        self.total_ignored += 1;
    }

    pub fn record_error(&mut self) {
        self.error_count += 1;
    }

    pub fn record_ai_call(&mut self, tokens: u64) {
        self.total_ai_calls += 1;
        self.total_tokens_used += tokens;
    }

    // ── 消息统计 — 新增方法 ───────────────────────────────────

    pub fn record_message_received_count(&mut self, count: u64) {
        self.total_messages_received += count;
    }

    pub fn record_reply_parts_sent(&mut self, parts: u64) {
        self.reply_parts_sent += parts;
    }

    pub fn record_message_error(&mut self, count: u64) {
        self.message_errors += count;
    }

    // ── 命令统计 — 新增方法 ───────────────────────────────────

    pub fn record_command_hit(&mut self, name: &str) {
        self.command_hits += 1;
        let normalized = if name.trim().is_empty() {
            "unknown"
        } else {
            name
        }
        .trim()
        .to_lowercase();
        *self.command_hits_by_name.entry(normalized).or_default() += 1;
    }

    // ── 规划器统计 — 新增方法 ─────────────────────────────────

    pub fn record_planner_action(&mut self, action: &str) {
        match action.trim().to_lowercase().as_str() {
            "reply" => self.planner_reply += 1,
            "wait" => self.planner_wait += 1,
            "ignore" => self.planner_ignore += 1,
            _ => {}
        }
    }

    // ── 视觉统计 — 新增方法 ───────────────────────────────────

    pub fn record_vision_request(
        &mut self,
        image_count: u64,
        failure_count: u64,
        reused_from_plan: bool,
    ) {
        if reused_from_plan {
            self.vision_reused_from_plan += 1;
            return;
        }
        self.vision_requests += 1;
        self.vision_images_processed += image_count;
        self.vision_failures += failure_count;
    }

    // ── 表情统计 — 新增方法 ───────────────────────────────────

    pub fn record_emoji_detection(&mut self, count: u64) {
        self.emoji_detected += count;
    }

    pub fn record_emoji_classification(&mut self, count: u64) {
        self.emoji_classified += count;
    }

    pub fn record_emoji_classification_failure(&mut self, count: u64) {
        self.emoji_classification_failures += count;
    }

    pub fn record_emoji_reply_decision(&mut self, count: u64) {
        self.emoji_reply_decisions += count;
    }

    pub fn record_emoji_reply_sent(&mut self, count: u64) {
        self.emoji_reply_sent += count;
    }

    pub fn record_emoji_reply_skipped(&mut self, count: u64) {
        self.emoji_reply_skipped += count;
    }

    pub fn record_emoji_reply_no_candidate(&mut self, count: u64) {
        self.emoji_reply_no_candidate += count;
    }

    pub fn record_emoji_total(&mut self, count: u64) {
        self.emoji_total += count;
    }

    // ── 记忆统计 — 新增方法 ───────────────────────────────────

    pub fn record_memory_read(&mut self, count: u64) {
        self.memory_reads += count;
    }

    pub fn record_memory_write(&mut self, count: u64) {
        self.memory_writes += count;
    }

    pub fn record_memory_access_denied(&mut self, count: u64) {
        self.memory_access_denied += count;
    }

    // ── 系统统计 — 新增方法 ───────────────────────────────────

    pub fn set_active_message_tasks(&mut self, count: u64) {
        self.active_message_tasks = count;
    }

    pub fn set_active_conversations(&mut self, count: u64) {
        self.active_conversations = count;
    }

    pub fn set_background_tasks(&mut self, count: u64) {
        self.background_tasks = count;
    }

    // ── 信号统计 — 新增方法 ───────────────────────────────────

    pub fn record_signal_l1_stale_invalidation(&mut self, count: u64) {
        self.signal_l1_stale_invalidation += count;
    }

    // ── 快照 ──────────────────────────────────────────────────

    /// 返回所有 u64 指标的快照
    pub fn snapshot(&self) -> HashMap<String, u64> {
        let mut map = HashMap::new();
        map.insert(
            "total_messages_received".to_string(),
            self.total_messages_received,
        );
        map.insert("total_replies_sent".to_string(), self.total_replies_sent);
        map.insert("total_ignored".to_string(), self.total_ignored);
        map.insert("error_count".to_string(), self.error_count);
        map.insert("total_ai_calls".to_string(), self.total_ai_calls);
        map.insert("total_tokens_used".to_string(), self.total_tokens_used);
        map.insert("reply_parts_sent".to_string(), self.reply_parts_sent);
        map.insert("message_errors".to_string(), self.message_errors);
        map.insert("command_hits".to_string(), self.command_hits);
        for (name, count) in &self.command_hits_by_name {
            map.insert(format!("cmd:{}", name), *count);
        }
        map.insert("planner_reply".to_string(), self.planner_reply);
        map.insert("planner_wait".to_string(), self.planner_wait);
        map.insert("planner_ignore".to_string(), self.planner_ignore);
        map.insert("vision_requests".to_string(), self.vision_requests);
        map.insert(
            "vision_images_processed".to_string(),
            self.vision_images_processed,
        );
        map.insert("vision_failures".to_string(), self.vision_failures);
        map.insert(
            "vision_reused_from_plan".to_string(),
            self.vision_reused_from_plan,
        );
        map.insert("emoji_detected".to_string(), self.emoji_detected);
        map.insert("emoji_classified".to_string(), self.emoji_classified);
        map.insert(
            "emoji_classification_failures".to_string(),
            self.emoji_classification_failures,
        );
        map.insert(
            "emoji_reply_decisions".to_string(),
            self.emoji_reply_decisions,
        );
        map.insert("emoji_reply_sent".to_string(), self.emoji_reply_sent);
        map.insert("emoji_reply_skipped".to_string(), self.emoji_reply_skipped);
        map.insert(
            "emoji_reply_no_candidate".to_string(),
            self.emoji_reply_no_candidate,
        );
        map.insert("emoji_total".to_string(), self.emoji_total);
        map.insert("memory_reads".to_string(), self.memory_reads);
        map.insert("memory_writes".to_string(), self.memory_writes);
        map.insert(
            "memory_access_denied".to_string(),
            self.memory_access_denied,
        );
        map.insert(
            "active_message_tasks".to_string(),
            self.active_message_tasks,
        );
        map.insert(
            "active_conversations".to_string(),
            self.active_conversations,
        );
        map.insert("background_tasks".to_string(), self.background_tasks);
        map.insert(
            "signal_l1_stale_invalidation".to_string(),
            self.signal_l1_stale_invalidation,
        );
        map
    }
}
