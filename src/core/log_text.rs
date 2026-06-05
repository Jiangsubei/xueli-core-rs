/// 日志文本预览工具 — 截断长文本用于日志输出。
///
/// 对应 Python 版 `xueli/src/core/log_text.py`
use regex::Regex;

/// 截断文本到指定长度，用于日志预览。
pub fn preview_text_for_log(text: &str, max_length: usize) -> String {
    let re = Regex::new(r"\s+").unwrap();
    let normalized = re.replace_all(text.trim(), " ").to_string();
    if normalized.len() <= max_length {
        normalized
    } else {
        let end = std::cmp::max(1, max_length.saturating_sub(3));
        format!("{}...", &normalized[..end])
    }
}

/// 将 JSON 可序列化数据截断为日志预览。
pub fn preview_json_for_log(data: &serde_json::Value, max_length: usize) -> String {
    let serialized = serde_json::to_string(data).unwrap_or_else(|_| format!("{:?}", data));
    preview_text_for_log(&serialized, max_length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preview_text_short() {
        let result = preview_text_for_log("hello world", 200);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_preview_text_long() {
        let long = "a".repeat(300);
        let result = preview_text_for_log(&long, 200);
        assert!(result.len() <= 200);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_preview_text_whitespace_normalized() {
        let result = preview_text_for_log("  hello   world  ", 200);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_preview_json() {
        let data = serde_json::json!({"key": "value", "arr": [1, 2, 3]});
        let result = preview_json_for_log(&data, 200);
        assert!(result.contains("key"));
    }
}
