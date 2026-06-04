use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// 待处理评估 — 等待用户反馈的回复
///
/// 对应 Python 版 `PendingEvaluation`
#[derive(Debug, Clone)]
pub struct PendingEvaluation {
    pub reply_text: String,
    pub reply_time: f64,
    pub target_user_id: String,
    pub group_id: String,
    pub reply_intent: String,
    pub expected_effect: String,
    pub predicted_response: String,
}

/// 回复效果评分
///
/// 对应 Python 版 `ReplyEffectScore`
#[derive(Debug, Clone, Default)]
pub struct ReplyEffectScore {
    pub score: f64,
    pub label: String,
    pub reply_intent: String,
    pub feedback_label: String,
    pub expected_effect: String,
    pub actual_effect: String,
    pub expected_effect_met: Option<bool>,
}

/// 回复效果追踪器 — 追踪每条回复的后续用户反馈
///
/// 对应 Python 版 `ReplyEffectTracker`
///
/// 观察同一用户接下来 2 条消息（10 分钟内），
/// 将待处理回复暴露给 feedback_triage。
/// 本模块有意不使用关键词推断语义反馈。
#[derive(Debug, Clone)]
pub struct ReplyEffectTracker {
    observation_window_seconds: f64,
    followup_limit: usize,
    pending: HashMap<String, PendingEvaluation>,
}

impl ReplyEffectTracker {
    pub fn new(observation_window_seconds: f64) -> Self {
        Self {
            observation_window_seconds,
            followup_limit: 2,
            pending: HashMap::new(),
        }
    }

    fn key(user_id: &str, group_id: &str) -> String {
        format!("{}:{}", group_id, user_id)
    }

    fn now(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }

    /// 记录一条发出的回复，等待后续用户反馈评估
    pub fn record_reply(
        &mut self,
        user_id: &str,
        group_id: &str,
        reply_text: &str,
        reply_intent: &str,
        expected_effect: &str,
        predicted_response: &str,
    ) {
        let key = Self::key(user_id, group_id);
        self.pending.insert(
            key,
            PendingEvaluation {
                reply_text: truncate_utf8(reply_text, 200),
                reply_time: self.now(),
                target_user_id: user_id.to_string(),
                group_id: group_id.to_string(),
                reply_intent: reply_intent.to_string(),
                expected_effect: normalize_expected_effect(expected_effect),
                predicted_response: predicted_response.to_string(),
            },
        );
    }

    /// 检查是否有待处理的回复评估
    pub fn has_pending(&self, user_id: &str, group_id: &str) -> bool {
        let key = Self::key(user_id, group_id);
        self.pending.get(&key).map_or(false, |p| {
            self.now() - p.reply_time <= self.observation_window_seconds
        })
    }

    /// 获取待处理回复的文本
    pub fn get_pending_reply(&self, user_id: &str, group_id: &str) -> String {
        let key = Self::key(user_id, group_id);
        self.pending
            .get(&key)
            .filter(|p| self.now() - p.reply_time <= self.observation_window_seconds)
            .map(|p| p.reply_text.clone())
            .unwrap_or_default()
    }

    /// 消费一次待处理评估（取出并移除，防止重复评分）
    pub fn consume_pending(&mut self, user_id: &str, group_id: &str) -> Option<PendingEvaluation> {
        let key = Self::key(user_id, group_id);
        let pending = self.pending.remove(&key)?;
        if self.now() - pending.reply_time > self.observation_window_seconds {
            return None;
        }
        Some(pending)
    }

    /// 清理过期的待处理条目
    pub fn cleanup(&mut self) {
        let now = self.now();
        self.pending
            .retain(|_, v| now - v.reply_time <= self.observation_window_seconds);
    }

    /// 活跃待处理条目数
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl Default for ReplyEffectTracker {
    fn default() -> Self {
        Self::new(600.0)
    }
}

/// 标准化预期效果标签
fn normalize_expected_effect(value: &str) -> String {
    let normalized = value.trim().to_lowercase();
    match normalized.as_str() {
        "continue" | "satisfy" | "cool_down" | "clarify" => normalized,
        _ => String::new(),
    }
}

/// 截断 UTF-8 字符串到指定字符数
fn truncate_utf8(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_normalize_expected_effect() {
        assert_eq!(normalize_expected_effect("continue"), "continue");
        assert_eq!(normalize_expected_effect("CONTINUE"), "continue");
        assert_eq!(normalize_expected_effect("  cool_down "), "cool_down");
        assert_eq!(normalize_expected_effect("unknown"), "");
        assert_eq!(normalize_expected_effect(""), "");
    }

    #[test]
    fn test_record_and_has_pending() {
        let mut tracker = ReplyEffectTracker::new(600.0);
        assert!(!tracker.has_pending("u1", "g1"));

        tracker.record_reply("u1", "g1", "你好呀", "greet", "continue", "用户会继续聊");
        assert!(tracker.has_pending("u1", "g1"));
        assert!(!tracker.has_pending("u2", "g1"));
    }

    #[test]
    fn test_consume_pending() {
        let mut tracker = ReplyEffectTracker::new(600.0);
        tracker.record_reply("u1", "g1", "回复内容", "answer", "satisfy", "");

        let pending = tracker.consume_pending("u1", "g1");
        assert!(pending.is_some());
        assert_eq!(pending.unwrap().reply_text, "回复内容");

        // 消费后不再存在
        assert!(!tracker.has_pending("u1", "g1"));
    }

    #[test]
    fn test_get_pending_reply() {
        let mut tracker = ReplyEffectTracker::new(600.0);
        tracker.record_reply("u1", "g1", "这条是回复", "", "", "");

        assert_eq!(tracker.get_pending_reply("u1", "g1"), "这条是回复");
        // get 不消费
        assert!(tracker.has_pending("u1", "g1"));
    }

    #[test]
    fn test_expiry() {
        // 使用极短窗口测试过期
        let mut tracker = ReplyEffectTracker::new(0.01);
        tracker.record_reply("u1", "g1", "会过期", "", "", "");

        thread::sleep(std::time::Duration::from_millis(20));

        assert!(!tracker.has_pending("u1", "g1"));
        assert!(tracker.consume_pending("u1", "g1").is_none());
    }

    #[test]
    fn test_cleanup() {
        let mut tracker = ReplyEffectTracker::new(0.01);
        tracker.record_reply("u1", "g1", "expire", "", "", "");
        tracker.record_reply("u2", "g2", "expire2", "", "", "");

        thread::sleep(std::time::Duration::from_millis(20));

        tracker.cleanup();
        assert_eq!(tracker.pending_count(), 0);
    }

    #[test]
    fn test_truncation() {
        let mut tracker = ReplyEffectTracker::new(600.0);
        let long_text = "a".repeat(300);
        tracker.record_reply("u1", "g1", &long_text, "", "", "");
        let pending = tracker.consume_pending("u1", "g1").unwrap();
        assert_eq!(pending.reply_text.chars().count(), 200);
    }

    #[test]
    fn test_default() {
        let tracker = ReplyEffectTracker::default();
        assert_eq!(tracker.pending_count(), 0);
    }
}
