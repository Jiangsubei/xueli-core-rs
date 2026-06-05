/// 对话时间线格式化器 — 为提示词编译和测试渲染时间线上下文。
///
/// 对应 Python 版 `xueli/src/handlers/conversation/timeline_formatter.py`
use std::collections::HashMap;

use crate::core::log_labels;
use crate::core::scope::ChatScope;
use crate::core::types::ConversationContextItem;
use crate::handlers::planner::PromptPlan;
use crate::handlers::shared::display_utils::window_display_text;
use crate::signals::temporal::TemporalContext;

/// 为提示词编译和测试渲染时间线上下文。
pub struct ConversationTimelineFormatter {
    /// LRU 缓存大小
    cache_maxsize: usize,
    build_items_cache: Vec<(String, Vec<ConversationContextItem>)>,
    render_cn_cache: Vec<(String, String)>,
}

impl ConversationTimelineFormatter {
    pub fn new() -> Self {
        Self {
            cache_maxsize: 4,
            build_items_cache: Vec::new(),
            render_cn_cache: Vec::new(),
        }
    }

    /// 生成缓存键
    fn make_cache_key(
        window_messages: &[HashMap<String, serde_json::Value>],
        extras: &[&str],
    ) -> String {
        let json = serde_json::to_string(window_messages).unwrap_or_default();
        let extra_part = extras.join("|");
        format!("{}|{}", json, extra_part)
    }

    fn cache_get<T: Clone>(cache: &mut Vec<(String, T)>, key: &str) -> Option<T> {
        if let Some(pos) = cache.iter().position(|(k, _)| k == key) {
            let (_, v) = cache.remove(pos);
            let result = v.clone();
            cache.push((key.to_string(), v));
            Some(result)
        } else {
            None
        }
    }

    fn cache_set<T>(cache: &mut Vec<(String, T)>, key: &str, value: T, maxsize: usize) {
        if let Some(pos) = cache.iter().position(|(k, _)| k == key) {
            cache.remove(pos);
        }
        cache.push((key.to_string(), value));
        while cache.len() > maxsize {
            cache.remove(0);
        }
    }

    /// 从窗口消息构建结构化上下文条目
    pub fn build_items(
        &mut self,
        window_messages: &[HashMap<String, serde_json::Value>],
    ) -> Vec<ConversationContextItem> {
        let key = Self::make_cache_key(window_messages, &[]);
        if let Some(cached) = Self::cache_get(&mut self.build_items_cache, &key) {
            return cached;
        }

        let mut items: Vec<ConversationContextItem> = Vec::new();
        for msg in window_messages {
            let text = msg
                .get("display_text")
                .or_else(|| msg.get("text"))
                .or_else(|| msg.get("raw_text"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "用户发送了空文本".to_string());

            let speaker_label = speaker_label(msg);
            let role = msg
                .get("speaker_role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();

            let timestamp = msg
                .get("event_time")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            let mut metadata = HashMap::new();
            if let Some(is_latest) = msg.get("is_latest").and_then(|v| v.as_bool()) {
                metadata.insert("is_latest".to_string(), serde_json::Value::Bool(is_latest));
            }

            items.push(ConversationContextItem {
                kind: "timeline_message".to_string(),
                text,
                role,
                speaker_label,
                timestamp,
                metadata,
                count_in_context: true,
            });
        }

        Self::cache_set(
            &mut self.build_items_cache,
            &key,
            items.clone(),
            self.cache_maxsize,
        );
        items
    }

    /// 渲染最近对话历史（摘要或逐条）
    pub fn render_recent_history(
        &self,
        window_messages: &[HashMap<String, serde_json::Value>],
        prompt_plan: Option<&PromptPlan>,
        temporal_context: Option<&TemporalContext>,
        chat_mode: &ChatScope,
        max_context_length: usize,
    ) -> String {
        let detail = prompt_plan
            .map(|p| p.timeline_detail.as_str())
            .unwrap_or("summary");
        if detail == "off" {
            return String::new();
        }

        let previous_items: Vec<&HashMap<String, serde_json::Value>> = window_messages
            .iter()
            .filter(|item| {
                !item
                    .get("is_latest")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .collect();

        if previous_items.is_empty() {
            return String::new();
        }

        let limit = if max_context_length > 0 {
            max_context_length
        } else {
            3
        };

        let reply_index = build_reply_index(window_messages);

        if detail == "summary" {
            let mut lines = vec!["最近上下文摘要：".to_string()];
            for item in previous_items.iter().rev().take(limit).rev() {
                let label = speaker_label(item);
                let prefix = reply_chain_prefix(item, &reply_index);
                let text = item
                    .get("display_text")
                    .or_else(|| item.get("text"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "用户发送了空文本".to_string());
                lines.push(format!("- {}：{}{}", label, prefix, text));
            }
            if let Some(tc) = temporal_context {
                if !tc.summary_text.is_empty() {
                    lines.push(format!("- 时间线观察：{}", tc.summary_text));
                }
            }
            return lines.join("\n");
        }

        // per_message 模式
        let mut lines = vec!["最近对话时间线：".to_string()];
        for item in previous_items.iter().rev().take(limit).rev() {
            let time_str = format_clock(
                item.get("event_time")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
            );
            let label = speaker_label(item);
            let prefix = reply_chain_prefix(item, &reply_index);
            let text = item
                .get("display_text")
                .or_else(|| item.get("text"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "用户发送了空文本".to_string());
            lines.push(format!("- [{}] {}：{}{}", time_str, label, prefix, text));
        }

        if chat_mode.is_group() && temporal_context.is_some_and(|tc| !tc.summary_text.is_empty()) {
            lines.push(format!(
                "- 时间线观察：{}",
                temporal_context.unwrap().summary_text
            ));
        }

        lines.join("\n")
    }

    /// 渲染时间上下文摘要
    pub fn render_summary(&self, temporal_context: Option<&TemporalContext>) -> String {
        let tc = match temporal_context {
            Some(tc) => tc,
            None => return String::new(),
        };
        if tc.summary_text.is_empty() {
            return String::new();
        }
        let recent_gap = &tc.recent_gap_bucket;
        let continuity = &tc.continuity_hint;
        format!(
            "{} 最近时间分层={}，连续性={}。",
            tc.summary_text, recent_gap, continuity
        )
    }

    /// 用固定的中文标签渲染面向模型的历史记录
    pub fn render_cn_model_history(
        &mut self,
        window_messages: &[HashMap<String, serde_json::Value>],
        current_message_text: &str,
        include_current: bool,
    ) -> String {
        let key = Self::make_cache_key(
            window_messages,
            &[current_message_text, &include_current.to_string()],
        );
        if let Some(cached) = Self::cache_get(&mut self.render_cn_cache, &key) {
            return cached;
        }

        let mut lines: Vec<String> = Vec::new();
        let reply_index = build_reply_index(window_messages);

        for item in window_messages {
            let timestamp = item
                .get("event_time")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let readable = format_readable_time(timestamp);
            let role = item
                .get("speaker_role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .trim()
                .to_lowercase();
            let text = window_display_text(item);
            let prefix = reply_chain_prefix(item, &reply_index);

            if role == "assistant" {
                lines.push(format!("时间 {} 你：{}{}", readable, prefix, text));
            } else {
                let speaker = speaker_label(item);
                lines.push(format!("时间 {} {}：{}{}", readable, speaker, prefix, text));
            }
        }

        let current = if current_message_text.trim().is_empty() {
            "用户发送了空文本".to_string()
        } else {
            current_message_text.trim().to_string()
        };

        if include_current {
            lines.push(format!("请你回复当前消息：{}", current));
        }

        let result = lines.join("\n");
        Self::cache_set(
            &mut self.render_cn_cache,
            &key,
            result.clone(),
            self.cache_maxsize,
        );
        result
    }
}

impl Default for ConversationTimelineFormatter {
    fn default() -> Self {
        Self::new()
    }
}

// --- 辅助函数 ---

/// 格式化时间戳为 HH:MM:SS
fn format_clock(timestamp: f64) -> String {
    if timestamp <= 0.0 {
        return "--:--:--".to_string();
    }
    let secs = timestamp as i64;
    let hours = (secs / 3600) % 24;
    let minutes = (secs / 60) % 60;
    let seconds = secs % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

/// 格式化可读时间
fn format_readable_time(timestamp: f64) -> String {
    if timestamp <= 0.0 {
        return "--".to_string();
    }
    let secs = timestamp as i64;
    // 使用 UTC 时间
    let days_since_epoch = secs / 86400;
    let remaining = secs % 86400;
    let h = remaining / 3600;
    let m = (remaining % 3600) / 60;
    let s = remaining % 60;
    // 简单计算日期
    let year = 1970 + (days_since_epoch / 365);
    let day_of_year = days_since_epoch % 365;
    let month = (day_of_year / 30) + 1;
    let day = (day_of_year % 30) + 1;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, h, m, s
    )
}

/// 确定发言者标签
fn speaker_label(item: &HashMap<String, serde_json::Value>) -> String {
    let role = item
        .get("speaker_role")
        .and_then(|v| v.as_str())
        .unwrap_or("user")
        .trim()
        .to_lowercase();

    if role == "assistant" {
        let name = item
            .get("speaker_name")
            .and_then(|v| v.as_str())
            .unwrap_or(log_labels::sender_labels::ASSISTANT)
            .trim();
        return if name.is_empty() {
            log_labels::sender_labels::ASSISTANT.to_string()
        } else {
            name.to_string()
        };
    }

    let speaker = item
        .get("speaker_name")
        .or_else(|| item.get("user_id"))
        .and_then(|v| v.as_str())
        .unwrap_or(log_labels::sender_labels::USER)
        .trim()
        .to_string();

    let user_id = item
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if !speaker.is_empty() && !user_id.is_empty() && speaker != user_id {
        format!("{}({})", speaker, user_id)
    } else if !speaker.is_empty() {
        speaker
    } else {
        user_id
    }
}

/// 构建回复索引：message_id → (speaker_label, text[:40])
fn build_reply_index(
    items: &[HashMap<String, serde_json::Value>],
) -> HashMap<String, (String, String)> {
    let mut index = HashMap::new();
    for item in items {
        if let Some(mid) = item.get("message_id").and_then(|v| v.as_str()) {
            if !mid.is_empty() {
                let speaker = speaker_label(item);
                let text = window_display_text(item);
                let truncated = if text.len() > 40 {
                    format!("{}...", &text[..40])
                } else {
                    text
                };
                index.insert(mid.to_string(), (speaker, truncated));
            }
        }
    }
    index
}

/// 生成回复链前缀
fn reply_chain_prefix(
    item: &HashMap<String, serde_json::Value>,
    msg_index: &HashMap<String, (String, String)>,
) -> String {
    let reply_to = item
        .get("reply_to_message_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if reply_to.is_empty() {
        return String::new();
    }
    match msg_index.get(reply_to) {
        Some((speaker, text)) => format!("[回复 {}: {}] ", speaker, text),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(
        role: &str,
        text: &str,
        speaker_name: &str,
        user_id: &str,
        event_time: f64,
    ) -> HashMap<String, serde_json::Value> {
        let mut m = HashMap::new();
        m.insert(
            "speaker_role".to_string(),
            serde_json::Value::String(role.to_string()),
        );
        m.insert(
            "text".to_string(),
            serde_json::Value::String(text.to_string()),
        );
        m.insert(
            "speaker_name".to_string(),
            serde_json::Value::String(speaker_name.to_string()),
        );
        m.insert(
            "user_id".to_string(),
            serde_json::Value::String(user_id.to_string()),
        );
        m.insert(
            "event_time".to_string(),
            serde_json::Value::Number(serde_json::Number::from_f64(event_time).unwrap()),
        );
        m.insert(
            "message_id".to_string(),
            serde_json::Value::String(format!("msg_{}", user_id)),
        );
        m
    }

    #[test]
    fn test_format_clock() {
        // 3600 seconds = 01:00:00
        let result = format_clock(3600.0);
        assert_eq!(result, "01:00:00");
    }

    #[test]
    fn test_format_clock_zero() {
        assert_eq!(format_clock(0.0), "--:--:--");
    }

    #[test]
    fn test_speaker_label_assistant() {
        let msg = make_msg("assistant", "你好", "雪梨", "bot1", 0.0);
        let label = speaker_label(&msg);
        assert!(label.contains("雪梨"));
    }

    #[test]
    fn test_speaker_label_user() {
        let msg = make_msg("user", "你好", "张三", "user123", 0.0);
        let label = speaker_label(&msg);
        assert!(label.contains("张三"));
    }

    #[test]
    fn test_build_items_empty() {
        let mut formatter = ConversationTimelineFormatter::new();
        let items = formatter.build_items(&[]);
        assert!(items.is_empty());
    }

    #[test]
    fn test_build_items_with_message() {
        let mut formatter = ConversationTimelineFormatter::new();
        let msgs = vec![make_msg("user", "你好世界", "张三", "u1", 1000.0)];
        let items = formatter.build_items(&msgs);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "你好世界");
        assert_eq!(items[0].kind, "timeline_message");
    }

    #[test]
    fn test_render_recent_history_summary() {
        let formatter = ConversationTimelineFormatter::new();
        let msgs = vec![
            {
                let mut m = make_msg("user", "你好", "张三", "u1", 1000.0);
                m.insert("is_latest".to_string(), serde_json::Value::Bool(false));
                m
            },
            {
                let mut m = make_msg("assistant", "你好呀", "雪梨", "bot1", 1010.0);
                m.insert("is_latest".to_string(), serde_json::Value::Bool(true));
                m
            },
        ];
        let result = formatter.render_recent_history(&msgs, None, None, &ChatScope::Private, 10);
        assert!(result.contains("最近上下文摘要"));
        assert!(result.contains("你好"));
    }

    #[test]
    fn test_render_recent_history_off() {
        let formatter = ConversationTimelineFormatter::new();
        let plan = PromptPlan {
            timeline_detail: "off".to_string(),
            ..PromptPlan::default()
        };
        let result =
            formatter.render_recent_history(&[], Some(&plan), None, &ChatScope::Private, 10);
        assert!(result.is_empty());
    }

    #[test]
    fn test_render_cn_model_history() {
        let mut formatter = ConversationTimelineFormatter::new();
        let msgs = vec![
            make_msg("user", "你好", "张三", "u1", 1000.0),
            make_msg("assistant", "你好呀", "雪梨", "bot1", 1010.0),
        ];
        let result = formatter.render_cn_model_history(&msgs, "今天天气怎么样", true);
        assert!(result.contains("时间"));
        assert!(result.contains("你"));
        assert!(result.contains("请你回复当前消息"));
        assert!(result.contains("今天天气怎么样"));
    }

    #[test]
    fn test_render_summary_empty() {
        let formatter = ConversationTimelineFormatter::new();
        let result = formatter.render_summary(None);
        assert!(result.is_empty());
    }
}
