/// 消息文本分段和格式化工具。
///
/// 对应 Python 版 `xueli/src/handlers/message_text.py`
use serde_json::Value;

/// 长消息拆分和文本提取。
pub struct MessageTextToolkit {
    max_message_length: usize,
}

impl MessageTextToolkit {
    pub fn new(max_message_length: usize) -> Self {
        Self {
            max_message_length: std::cmp::max(1, max_message_length),
        }
    }

    /// 提取并归一化用户消息文本。
    pub fn extract(&self, text: &str) -> String {
        text.trim().to_string()
    }

    /// 将长消息拆分为不超过最大长度的片段。
    pub fn split_long_message(&self, message: &str) -> Vec<String> {
        if message.len() <= self.max_message_length {
            return vec![message.to_string()];
        }

        let mut parts: Vec<String> = Vec::new();
        let mut current = String::new();

        for line in message.split('\n') {
            if line.len() > self.max_message_length {
                // 将当前累积的段落先推入
                if !current.is_empty() {
                    parts.push(current.clone());
                    current.clear();
                }
                // 按最大长度切分超长行
                for idx in (0..line.len()).step_by(self.max_message_length) {
                    let end = std::cmp::min(idx + self.max_message_length, line.len());
                    parts.push(line[idx..end].to_string());
                }
                continue;
            }

            if current.len() + line.len() + 1 > self.max_message_length {
                if !current.is_empty() {
                    parts.push(current.clone());
                }
                current = line.to_string();
            } else if current.is_empty() {
                current = line.to_string();
            } else {
                current = format!("{}\n{}", current, line);
            }
        }

        if !current.is_empty() {
            parts.push(current);
        }

        parts
    }

    /// 格式化消息内容用于提示词展示（文本/列表/字典）。
    pub fn format_prompt_content(content: &Value) -> String {
        match content {
            Value::String(s) => s.clone(),
            Value::Array(arr) => {
                let mut lines: Vec<String> = Vec::new();
                let mut image_count = 0usize;
                for part in arr {
                    match part {
                        Value::Object(obj) => {
                            if obj.get("type").and_then(|v| v.as_str()) == Some("text") {
                                if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                                    if !text.is_empty() {
                                        lines.push(text.to_string());
                                    }
                                }
                            } else if obj.get("type").and_then(|v| v.as_str()) == Some("image_url")
                            {
                                image_count += 1;
                            }
                        }
                        other => {
                            lines.push(other.to_string());
                        }
                    }
                }
                if image_count > 0 {
                    lines.push(format!("[图片 {} 张]", image_count));
                }
                if lines.is_empty() {
                    content.to_string()
                } else {
                    lines.join("\n")
                }
            }
            _ => content.to_string(),
        }
    }
}

impl Default for MessageTextToolkit {
    fn default() -> Self {
        Self::new(2048)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract() {
        let toolkit = MessageTextToolkit::default();
        assert_eq!(toolkit.extract("  hello  "), "hello");
        assert_eq!(toolkit.extract(""), "");
    }

    #[test]
    fn test_split_short() {
        let toolkit = MessageTextToolkit::new(100);
        let result = toolkit.split_long_message("短消息");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "短消息");
    }

    #[test]
    fn test_split_long() {
        let toolkit = MessageTextToolkit::new(10);
        let result = toolkit.split_long_message("12345678901");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "1234567890");
        assert_eq!(result[1], "1");
    }

    #[test]
    fn test_split_long_multiline() {
        let toolkit = MessageTextToolkit::new(20);
        let msg = "First line content\nSecond line here\nThird line is very long and exceeds the max length";
        let result = toolkit.split_long_message(msg);
        assert!(result.len() >= 2);
    }

    #[test]
    fn test_format_prompt_content_text() {
        let content = serde_json::json!("hello");
        let result = MessageTextToolkit::format_prompt_content(&content);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_format_prompt_content_array() {
        let content = serde_json::json!([
            {"type": "text", "text": "你好"},
            {"type": "image_url", "image_url": {"url": "http://example.com/img.png"}},
        ]);
        let result = MessageTextToolkit::format_prompt_content(&content);
        assert!(result.contains("你好"));
        assert!(result.contains("图片"));
    }
}
