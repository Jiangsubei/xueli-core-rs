// 统一消息历史渲染 — Timing Gate 和 Planner 共用
// 对应 Python 版 xueli/src/handlers/shared/unified_history_renderer.py

use chrono::{TimeZone, Utc};

/// 统一历史消息条目
#[derive(Debug, Clone)]
pub struct UnifiedHistoryItem {
    pub timestamp: f64,
    pub role: String,
    pub content: String,
}

/// 将统一历史消息列表渲染为文本
///
/// 输出格式：
/// ```
/// 时间 2024-01-01 12:00:00 用户：你好
/// 时间 2024-01-01 12:00:05 你：你好呀
/// ...
/// 请你回复当前消息：{current_message_text}
/// ```
pub fn render_unified_history(
    unified_history: &[UnifiedHistoryItem],
    current_message_text: &str,
    include_current: bool,
    max_items: usize,
) -> String {
    let items: Vec<&UnifiedHistoryItem> = if max_items > 0 && unified_history.len() > max_items {
        unified_history[unified_history.len() - max_items..]
            .iter()
            .collect()
    } else {
        unified_history.iter().collect()
    };

    let mut lines: Vec<String> = Vec::new();
    for item in &items {
        let time_str = if item.timestamp > 0.0 {
            match Utc.timestamp_opt(item.timestamp as i64, 0) {
                chrono::offset::LocalResult::Single(dt) => {
                    format!("{}", dt.format("%Y-%m-%d %H:%M:%S"))
                }
                _ => "--".to_string(),
            }
        } else {
            "--".to_string()
        };

        let content = item.content.trim();
        if content.is_empty() {
            continue;
        }

        let speaker = if item.role.trim().to_lowercase() == "assistant" {
            "你"
        } else {
            "用户"
        };

        lines.push(format!("时间 {} {}：{}", time_str, speaker, content));
    }

    if include_current && !current_message_text.is_empty() {
        lines.push(format!("请你回复当前消息：{}", current_message_text));
    }

    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_unified_history_basic() {
        let history = vec![
            UnifiedHistoryItem {
                timestamp: 1704067200.0, // 2024-01-01 00:00:00 UTC
                role: "user".into(),
                content: "你好".into(),
            },
            UnifiedHistoryItem {
                timestamp: 1704067205.0,
                role: "assistant".into(),
                content: "你好呀".into(),
            },
        ];
        let result = render_unified_history(&history, "", true, 0);
        assert!(result.contains("用户：你好"));
        assert!(result.contains("你：你好呀"));
    }

    #[test]
    fn test_render_unified_history_with_current() {
        let history = vec![];
        let result = render_unified_history(&history, "今天天气真好", true, 0);
        assert_eq!(result, "请你回复当前消息：今天天气真好");
    }

    #[test]
    fn test_render_unified_history_empty() {
        let result = render_unified_history(&[], "", false, 0);
        assert_eq!(result, "");
    }

    #[test]
    fn test_render_unified_history_max_items() {
        let mut history = Vec::new();
        for i in 0..5 {
            history.push(UnifiedHistoryItem {
                timestamp: 1704067200.0 + i as f64,
                role: "user".into(),
                content: format!("消息{}", i),
            });
        }
        let result = render_unified_history(&history, "", false, 3);
        // 应只保留最后 3 条
        assert!(!result.contains("消息0"));
        assert!(!result.contains("消息1"));
        assert!(result.contains("消息2"));
        assert!(result.contains("消息4"));
    }

    #[test]
    fn test_render_unified_history_skip_empty_content() {
        let history = vec![
            UnifiedHistoryItem {
                timestamp: 1704067200.0,
                role: "user".into(),
                content: "   ".into(),
            },
            UnifiedHistoryItem {
                timestamp: 1704067205.0,
                role: "assistant".into(),
                content: "有效内容".into(),
            },
        ];
        let result = render_unified_history(&history, "", false, 0);
        assert!(!result.contains("用户："));
        assert!(result.contains("有效内容"));
    }
}
