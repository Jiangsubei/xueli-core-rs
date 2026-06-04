// 时间上下文信号 — 规划和提示词编译使用的时间间隔信号
///
/// 对应 Python 版 `xueli/src/handlers/signals/temporal_context.py`
#[derive(Debug, Clone)]
pub struct TemporalContext {
    pub current_event_time: f64,
    pub previous_message_time: f64,
    pub conversation_last_time: f64,
    pub previous_session_time: f64,
    pub recent_gap_seconds: Option<f64>,
    pub conversation_gap_seconds: Option<f64>,
    pub session_gap_seconds: Option<f64>,
    pub history_span_seconds: Option<f64>,
    pub recent_gap_bucket: String,
    pub conversation_gap_bucket: String,
    pub session_gap_bucket: String,
    pub continuity_hint: String,
    pub summary_text: String,
}

impl TemporalContext {
    pub fn new() -> Self {
        Self {
            current_event_time: 0.0,
            previous_message_time: 0.0,
            conversation_last_time: 0.0,
            previous_session_time: 0.0,
            recent_gap_seconds: None,
            conversation_gap_seconds: None,
            session_gap_seconds: None,
            history_span_seconds: None,
            recent_gap_bucket: "unknown".to_string(),
            conversation_gap_bucket: "unknown".to_string(),
            session_gap_bucket: "unknown".to_string(),
            continuity_hint: "unknown".to_string(),
            summary_text: String::new(),
        }
    }
}

impl Default for TemporalContext {
    fn default() -> Self {
        Self::new()
    }
}

// --- 时间规范化 ---

/// 规范化事件时间戳（秒级浮点数）
pub fn normalize_event_time(raw_value: f64) -> f64 {
    if raw_value <= 0.0 {
        return 0.0;
    }
    if raw_value > 1_000_000_000_000.0 {
        return raw_value / 1000.0;
    }
    raw_value
}

// --- 间隔分桶 ---

fn gap_thresholds(chat_mode: &str) -> Vec<(f64, &'static str)> {
    let normalized = chat_mode.to_lowercase();
    if normalized == "group" {
        vec![
            (30.0, "immediate"),
            (180.0, "very_recent"),
            (900.0, "recent"),
            (3600.0, "same_day_resume"),
            (21600.0, "late_same_day"),
            (86400.0, "short_resume"),
            (259200.0, "long_resume"),
        ]
    } else {
        vec![
            (60.0, "immediate"),
            (600.0, "very_recent"),
            (3600.0, "recent"),
            (21600.0, "same_day_resume"),
            (86400.0, "late_same_day"),
            (259200.0, "short_resume"),
            (604800.0, "long_resume"),
        ]
    }
}

/// 将间隔秒数映射到时间分层桶
pub fn bucket_gap(seconds: Option<f64>, chat_mode: &str) -> String {
    let seconds = match seconds {
        Some(s) if s >= 0.0 => s,
        _ => return "unknown".to_string(),
    };
    for (threshold, label) in gap_thresholds(chat_mode) {
        if seconds < threshold {
            return label.to_string();
        }
    }
    "stale".to_string()
}

/// 连续性提示
pub fn continuity_hint(recent_bucket: &str, conversation_bucket: &str) -> String {
    let candidate = if recent_bucket != "unknown" {
        recent_bucket
    } else {
        conversation_bucket
    };
    match candidate {
        "immediate" | "very_recent" => "strong_continuation",
        "recent" => "soft_continuation",
        "same_day_resume" | "late_same_day" => "resume_after_break",
        "short_resume" | "long_resume" | "stale" => "old_topic_resume",
        _ => "unknown",
    }
    .to_string()
}

fn bucket_observation(bucket: &str) -> &str {
    match bucket {
        "immediate" => "当前消息和最近一条上下文消息几乎连在一起",
        "very_recent" => "当前消息和最近一条上下文消息间隔很短",
        "recent" => "当前消息和最近一条上下文消息间隔不久",
        "same_day_resume" => "当前消息和最近一条上下文消息之间已经有一段同日间隔",
        "late_same_day" => "当前消息和最近一条上下文消息之间已经有较明显的同日间隔",
        "short_resume" => "当前消息和最近一条上下文消息之间已隔了 1 到 3 天左右",
        "long_resume" => "当前消息和最近一条上下文消息之间已隔了数天",
        "stale" => "当前消息和最近一条上下文消息之间已经间隔较久",
        _ => "当前缺少足够的最近消息时间信息",
    }
}

/// 生成时间上下文摘要文本
pub fn summarize_temporal_context(ctx: &TemporalContext) -> String {
    let mut lines: Vec<String> = Vec::new();
    if ctx.session_gap_bucket != "unknown" {
        lines.push(format!(
            "最近一条历史消息时间分层是 {}",
            ctx.recent_gap_bucket
        ));
        lines.push(format!(
            "上一轮已关闭会话的时间分层是 {}",
            ctx.session_gap_bucket
        ));
    } else {
        lines.push(bucket_observation(&ctx.recent_gap_bucket).to_string());
    }
    if let Some(span) = ctx.history_span_seconds {
        if span < 300.0 {
            lines.push("最近窗口里的消息时间分布比较集中".to_string());
        } else if span < 7200.0 {
            lines.push("最近窗口里的消息覆盖了同一段较短时间范围".to_string());
        } else {
            lines.push("最近窗口里的消息跨越了较长时间范围".to_string());
        }
    }
    lines.join("；")
}

/// 构建时间上下文（从原始时间戳）
///
/// 对应 Python 版 `build_temporal_context()`
pub fn build_temporal_context(
    current_event_time: f64,
    chat_mode: &str,
    previous_message_time: f64,
    conversation_last_time: f64,
    previous_session_time: f64,
    history_event_times: &[f64],
) -> TemporalContext {
    let current_time = if current_event_time > 0.0 {
        normalize_event_time(current_event_time)
    } else {
        chrono::Utc::now().timestamp() as f64
    };
    let previous_time = normalize_event_time(previous_message_time);
    let conversation_time = normalize_event_time(conversation_last_time);
    let session_time = normalize_event_time(previous_session_time);

    let recent_gap_seconds = if previous_time > 0.0 {
        Some(current_time - previous_time)
    } else {
        None
    };
    let conversation_gap_seconds = if conversation_time > 0.0 {
        Some(current_time - conversation_time)
    } else {
        None
    };
    let session_gap_seconds = if session_time > 0.0 {
        Some(current_time - session_time)
    } else {
        None
    };

    let normalized_history: Vec<f64> = history_event_times
        .iter()
        .copied()
        .map(normalize_event_time)
        .filter(|&t| t > 0.0)
        .collect();
    let history_span_seconds = if normalized_history.len() >= 2 {
        let min_t = normalized_history
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        let max_t = normalized_history
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        Some(max_t - min_t)
    } else {
        None
    };

    let chat_mode_str = if chat_mode == "group" {
        "group"
    } else {
        "private"
    };
    let recent_gap_bucket = bucket_gap(recent_gap_seconds, chat_mode_str);
    let conversation_gap_bucket = bucket_gap(conversation_gap_seconds, chat_mode_str);
    let session_gap_bucket = bucket_gap(session_gap_seconds, chat_mode_str);

    let mut result = TemporalContext {
        current_event_time: current_time,
        previous_message_time: previous_time,
        conversation_last_time: conversation_time,
        previous_session_time: session_time,
        recent_gap_seconds,
        conversation_gap_seconds,
        session_gap_seconds,
        history_span_seconds,
        recent_gap_bucket: recent_gap_bucket.clone(),
        conversation_gap_bucket,
        session_gap_bucket,
        continuity_hint: String::new(),
        summary_text: String::new(),
    };
    result.continuity_hint = continuity_hint(&recent_gap_bucket, &result.conversation_gap_bucket);
    result.summary_text = summarize_temporal_context(&result);
    result
}

/// 从 InboundEvent 构建时间上下文
///
/// 对应 Python 版 `build_temporal_context_from_event()`
pub fn build_temporal_context_from_event(
    event_timestamp: f64,
    is_group: bool,
    previous_message_time: f64,
    conversation_last_time: f64,
    previous_session_time: f64,
    history_event_times: &[f64],
) -> TemporalContext {
    let chat_mode = if is_group { "group" } else { "private" };
    build_temporal_context(
        event_timestamp,
        chat_mode,
        previous_message_time,
        conversation_last_time,
        previous_session_time,
        history_event_times,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_event_time_ms() {
        // 毫秒级时间戳
        let result = normalize_event_time(1_700_000_000_000.0);
        assert!((result - 1_700_000_000.0).abs() < 1.0);
    }

    #[test]
    fn test_normalize_event_time_seconds() {
        let result = normalize_event_time(1_700_000_000.0);
        assert!((result - 1_700_000_000.0).abs() < 1.0);
    }

    #[test]
    fn test_normalize_event_time_zero() {
        assert_eq!(normalize_event_time(0.0), 0.0);
    }

    #[test]
    fn test_bucket_gap_private_immediate() {
        let bucket = bucket_gap(Some(30.0), "private");
        assert_eq!(bucket, "immediate");
    }

    #[test]
    fn test_bucket_gap_private_very_recent() {
        let bucket = bucket_gap(Some(300.0), "private");
        assert_eq!(bucket, "very_recent");
    }

    #[test]
    fn test_bucket_gap_group_immediate() {
        let bucket = bucket_gap(Some(15.0), "group");
        assert_eq!(bucket, "immediate");
    }

    #[test]
    fn test_bucket_gap_stale() {
        let bucket = bucket_gap(Some(1_000_000.0), "private");
        assert_eq!(bucket, "stale");
    }

    #[test]
    fn test_bucket_gap_none() {
        let bucket = bucket_gap(None, "private");
        assert_eq!(bucket, "unknown");
    }

    #[test]
    fn test_continuity_hint_strong() {
        let hint = continuity_hint("immediate", "unknown");
        assert_eq!(hint, "strong_continuation");
    }

    #[test]
    fn test_continuity_hint_soft() {
        let hint = continuity_hint("recent", "unknown");
        assert_eq!(hint, "soft_continuation");
    }

    #[test]
    fn test_continuity_hint_resume() {
        let hint = continuity_hint("same_day_resume", "unknown");
        assert_eq!(hint, "resume_after_break");
    }

    #[test]
    fn test_continuity_hint_old_topic() {
        let hint = continuity_hint("long_resume", "unknown");
        assert_eq!(hint, "old_topic_resume");
    }

    #[test]
    fn test_build_temporal_context_basic() {
        let ctx = build_temporal_context(
            1_700_000_000.0,
            "private",
            1_699_999_985.0,
            1_699_999_000.0,
            0.0,
            &[1_699_999_985.0, 1_700_000_000.0],
        );
        assert_eq!(ctx.recent_gap_bucket, "immediate");
        assert_eq!(ctx.continuity_hint, "strong_continuation");
        assert!(!ctx.summary_text.is_empty());
        assert!(ctx.history_span_seconds.is_some());
    }

    #[test]
    fn test_build_temporal_context_from_event() {
        let ctx = build_temporal_context_from_event(
            1_700_000_000.0,
            false,
            1_699_999_980.0,
            1_699_999_000.0,
            0.0,
            &[1_699_999_900.0, 1_700_000_000.0],
        );
        assert_eq!(ctx.recent_gap_bucket, "immediate");
    }

    #[test]
    fn test_summarize_temporal_context_with_session() {
        let mut ctx = TemporalContext::new();
        ctx.recent_gap_bucket = "immediate".to_string();
        ctx.session_gap_bucket = "short_resume".to_string();
        ctx.history_span_seconds = Some(50.0);
        let summary = summarize_temporal_context(&ctx);
        assert!(summary.contains("最近一条历史消息时间分层"));
        assert!(summary.contains("上一轮已关闭会话"));
        assert!(summary.contains("集中"));
    }
}
