use std::collections::HashMap;
use std::collections::HashSet;

use regex::Regex;

use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};
use crate::prelude::XueliResult;

/// 聊天摘要服务 — 为会话构建紧凑摘要
pub struct ChatSummaryService {
    max_points: usize,
    max_point_chars: usize,
    max_summary_chars: usize,
    whitespace_re: Regex,
}

impl Default for ChatSummaryService {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatSummaryService {
    pub fn new() -> Self {
        Self {
            max_points: 3,
            max_point_chars: 72,
            max_summary_chars: 220,
            whitespace_re: Regex::new(r"\s+").expect("正则编译失败"),
        }
    }

    /// 设置最大要点数
    pub fn with_max_points(mut self, max: usize) -> Self {
        self.max_points = max;
        self
    }

    /// 设置每个要点最大字符数
    pub fn with_max_point_chars(mut self, max: usize) -> Self {
        self.max_point_chars = max;
        self
    }

    /// 设置摘要最大字符数
    pub fn with_max_summary_chars(mut self, max: usize) -> Self {
        self.max_summary_chars = max;
        self
    }

    /// 从会话记录构建摘要
    pub fn build_summary(&self, records: &[ConversationRecord]) -> String {
        if records.is_empty() {
            return String::new();
        }

        let mut points = Vec::new();
        let mut seen = HashSet::new();

        // 从最近的消息开始收集用户发言要点
        for record in records.iter().rev() {
            if record.is_bot {
                continue;
            }

            let text = self.normalize_fragment(&record.text);
            if !text.is_empty() && !seen.contains(&text) {
                seen.insert(text.clone());
                points.push(text);

                if points.len() >= self.max_points.max(1) {
                    break;
                }
            }
        }

        points.reverse();

        if points.is_empty() {
            // 如果没有用户发言，取最后一条助手回复
            if let Some(last) = records.last() {
                if last.is_bot {
                    return self.truncate_text(&last.text, self.max_summary_chars);
                }
            }
            return String::new();
        }

        let summary = points.join("；");
        self.truncate_text(&summary, self.max_summary_chars)
    }

    /// 获取会话的摘要（如果已有缓存则返回缓存）
    pub fn get_summary(
        &self,
        records: &[ConversationRecord],
        cached_summary: Option<&str>,
    ) -> String {
        if let Some(summary) = cached_summary {
            let s = summary.trim();
            if !s.is_empty() {
                return s.to_string();
            }
        }
        self.build_summary(records)
    }

    /// 刷新会话摘要并更新存储
    pub async fn refresh_session_summary(
        &self,
        store: &SqliteConversationStore,
        session_id: &str,
        _user_id: &str,
    ) -> XueliResult<Option<String>> {
        let records = store.get_recent_by_session(session_id, 1000).await?;
        if records.is_empty() {
            return Ok(None);
        }

        let summary = self.build_summary(&records);
        if summary.is_empty() {
            return Ok(None);
        }

        let turn_count = records.len();
        let mut metadata = HashMap::new();
        metadata.insert("session_summary".to_string(), summary.clone());
        metadata.insert(
            "session_summary_turn_count".to_string(),
            turn_count.to_string(),
        );

        store.update_session_metadata(session_id, &metadata).await?;

        Ok(Some(summary))
    }

    /// 归一化文本片段
    fn normalize_fragment(&self, text: &str) -> String {
        let normalized = self.whitespace_re.replace_all(text.trim(), " ");
        let normalized = normalized.trim();

        if normalized.is_empty() {
            return String::new();
        }

        self.truncate_text(normalized, self.max_point_chars)
    }

    /// 截断文本到指定长度
    fn truncate_text(&self, text: &str, max_len: usize) -> String {
        if text.len() <= max_len {
            return text.to_string();
        }

        let trunc_len = max_len.saturating_sub(3).max(1);
        let mut result = text.chars().take(trunc_len).collect::<String>();
        result = result.trim_end().to_string();
        result.push_str("...");
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(text: &str, is_bot: bool, event_time: f64) -> ConversationRecord {
        ConversationRecord {
            id: 0,
            session_id: "test_session".to_string(),
            user_id: "user1".to_string(),
            sender_name: if is_bot {
                "bot".to_string()
            } else {
                "user".to_string()
            },
            text: text.to_string(),
            is_bot,
            scope_type: "private".to_string(),
            scope_id: String::new(),
            event_time,
            message_id: format!("msg_{}", event_time as i64),
            platform: "qq".to_string(),
        }
    }

    #[test]
    fn test_build_summary_basic() {
        let service = ChatSummaryService::new();
        let records = vec![
            make_record("你好", false, 1.0),
            make_record("你好呀", true, 2.0),
            make_record("今天天气不错", false, 3.0),
            make_record("是的，很适合出门", true, 4.0),
        ];

        let summary = service.build_summary(&records);
        assert!(!summary.is_empty());
        assert!(summary.contains("你好") || summary.contains("天气"));
    }

    #[test]
    fn test_build_summary_empty() {
        let service = ChatSummaryService::new();
        let records: Vec<ConversationRecord> = vec![];
        assert_eq!(service.build_summary(&records), "");
    }

    #[test]
    fn test_build_summary_only_bot() {
        let service = ChatSummaryService::new();
        let records = vec![
            make_record("你好", true, 1.0),
            make_record("有什么可以帮你的", true, 2.0),
        ];

        let summary = service.build_summary(&records);
        assert_eq!(summary, "有什么可以帮你的");
    }

    #[test]
    fn test_truncate_text() {
        let service = ChatSummaryService::new().with_max_summary_chars(10);
        let long_text = "这是一段非常长的文本需要被截断";
        let truncated = service.truncate_text(long_text, 10);
        // 中文字符截断后长度可能超过字节限制，但字符数应该合理
        assert!(truncated.ends_with("..."));
        assert!(truncated.chars().count() <= 10);
    }

    #[test]
    fn test_normalize_fragment() {
        let service = ChatSummaryService::new();
        let text = "  多个   空格   需要  合并  ";
        let normalized = service.normalize_fragment(text);
        assert_eq!(normalized, "多个 空格 需要 合并");
    }

    #[test]
    fn test_deduplication() {
        let service = ChatSummaryService::new();
        let records = vec![
            make_record("重复消息", false, 1.0),
            make_record("重复消息", false, 2.0),
            make_record("新消息", false, 3.0),
        ];

        let summary = service.build_summary(&records);
        // 应该只出现一次"重复消息"
        let count = summary.matches("重复消息").count();
        assert_eq!(count, 1);
    }
}
