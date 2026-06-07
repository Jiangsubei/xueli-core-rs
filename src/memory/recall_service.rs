use std::sync::Arc;

use crate::core::types::MemoryItem;
use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};
use crate::prelude::XueliResult;

/// 会话记忆回忆服务 — 从历史对话中检索与当前话题相关的轮次。
pub struct ConversationRecallService {
    store: Arc<SqliteConversationStore>,
    recent_session_limit: usize,
    recall_entry_limit: usize,
    min_match_score: f64,
    max_excerpt_chars: usize,
    #[allow(dead_code)]
    recall_confidence_decay_per_day: f64,
    #[allow(dead_code)]
    recall_confidence_minimum: f64,
}

/// 回忆条目
#[derive(Debug, Clone)]
pub struct RecallEntry {
    pub content: String,
    pub session_id: String,
    pub dialogue_key: String,
    pub turn_id: usize,
    pub score: f64,
    pub recall_confidence: f64,
}

/// 回忆到的单轮
#[derive(Debug, Clone)]
struct ScoredTurn {
    record: ConversationRecord,
    #[allow(dead_code)]
    turn_user: String,
    #[allow(dead_code)]
    turn_assistant: String,
    #[allow(dead_code)]
    turn_timestamp: String,
    #[allow(dead_code)]
    turn_id: usize,
    #[allow(dead_code)]
    score: f64,
    #[allow(dead_code)]
    recall_confidence: f64,
}

impl ConversationRecallService {
    pub fn new(store: Arc<SqliteConversationStore>) -> Self {
        Self {
            store,
            recent_session_limit: 12,
            recall_entry_limit: 2,
            min_match_score: 0.18,
            max_excerpt_chars: 72,
            recall_confidence_decay_per_day: 0.01,
            recall_confidence_minimum: 0.3,
        }
    }

    /// 按时间衰减计算置信度
    #[allow(dead_code)]
    fn compute_confidence(&self, timestamp: &str) -> f64 {
        if timestamp.is_empty() {
            return 1.0;
        }
        let dt = match chrono::DateTime::parse_from_rfc3339(timestamp) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => return 1.0,
        };
        let now = chrono::Utc::now();
        let days = (now - dt).num_seconds() as f64 / 86400.0;
        let confidence = 1.0 - days * self.recall_confidence_decay_per_day;
        if confidence > self.recall_confidence_minimum {
            confidence
        } else {
            self.recall_confidence_minimum
        }
    }

    /// 召回与查询相关的历史记忆条目
    pub async fn recall(
        &self,
        user_id: &str,
        _session_id: &str,
        query: &str,
    ) -> XueliResult<Vec<MemoryItem>> {
        let normalized = Self::normalize_text(query);
        if normalized.len() < 2 {
            return Ok(vec![]);
        }

        let records =
            self.store
                .get_recent_by_scope("private", user_id, self.recent_session_limit)?;

        let mut matches: Vec<ScoredTurn> = Vec::new();
        for record in records.iter().rev() {
            // 简化：在 text 中匹配
            let score = self.score_turn_text(query, &record.text);
            if score < self.min_match_score {
                continue;
            }
            let confidence = 1.0; // 无精确时间戳时默认满分
            matches.push(ScoredTurn {
                record: record.clone(),
                turn_user: record.sender_name.clone(),
                turn_assistant: if record.is_bot {
                    record.text.clone()
                } else {
                    String::new()
                },
                turn_timestamp: String::new(),
                turn_id: record.id as usize,
                score,
                recall_confidence: confidence,
            });
        }

        if matches.is_empty() {
            return Ok(vec![]);
        }

        let selected: Vec<&ScoredTurn> = matches.iter().take(self.recall_entry_limit).collect();

        let items: Vec<MemoryItem> = selected
            .iter()
            .enumerate()
            .map(|(idx, m)| {
                let label = if idx == 0 {
                    "与当前话题相关的历史"
                } else {
                    "前述话题的延续"
                };
                let content = format!(
                    "{}：{}说“{}”",
                    label,
                    Self::excerpt(&m.turn_user, self.max_excerpt_chars),
                    Self::excerpt(&m.turn_assistant, self.max_excerpt_chars),
                );
                MemoryItem {
                    id: format!("recall_{}", m.record.id),
                    user_id: user_id.to_string(),
                    content,
                    memory_type: crate::core::types::MemoryType::Event,
                    importance: 0.5,
                    created_at: chrono::Utc::now(),
                    last_accessed_at: chrono::Utc::now(),
                    access_count: 0,
                }
            })
            .collect();

        Ok(items)
    }

    /// 对一条记录文本和查询进行匹配打分
    fn score_turn_text(&self, query: &str, text: &str) -> f64 {
        let q = Self::normalize_text(query);
        let t = Self::normalize_text(text);
        if q.is_empty() || t.is_empty() {
            return 0.0;
        }
        // 子串匹配
        if t.contains(&q) || q.contains(&t) {
            return q.len().min(t.len()) as f64 / q.len().max(t.len()) as f64;
        }
        // 字符重叠
        let q_chars: std::collections::HashSet<char> = q.chars().collect();
        let t_chars: std::collections::HashSet<char> = t.chars().collect();
        let overlap = q_chars.intersection(&t_chars).count();
        overlap as f64 / q_chars.len().max(1) as f64
    }

    /// 归一化文本：小写、去空白、保留字母数字和中文字符
    fn normalize_text(text: &str) -> String {
        let compact: String = text
            .trim()
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        compact
            .chars()
            .filter(|c| c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
            .collect()
    }

    /// 截取文本摘要
    fn excerpt(text: &str, max_chars: usize) -> String {
        let compact: String = text.trim().chars().filter(|c| !c.is_whitespace()).collect();
        if compact.chars().count() <= max_chars {
            return compact;
        }
        let truncated: String = compact.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{}...", truncated)
    }
}

impl Default for ConversationRecallService {
    fn default() -> Self {
        // 使用默认路径
        let dir = std::path::PathBuf::from("data/conversations");
        let store =
            Arc::new(SqliteConversationStore::open(&dir).expect("无法打开默认 ConversationStore"));
        Self::new(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_service() -> ConversationRecallService {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SqliteConversationStore::open(dir.path()).unwrap());
        ConversationRecallService::new(store)
    }

    #[test]
    fn test_normalize_text() {
        let result = ConversationRecallService::normalize_text("Hello 世界！Test 123");
        assert!(result.contains("hello"));
        assert!(result.contains("世界"));
        assert!(result.contains("test"));
        assert!(result.contains("123"));
        // 不应含空白和标点
        assert!(!result.contains(' '));
        assert!(!result.contains('！'));
    }

    #[test]
    fn test_score_text_substring_match() {
        let srv = make_service();
        let score = srv.score_turn_text("你好", "你好世界！");
        assert!(score >= 0.5);
    }

    #[test]
    fn test_score_text_no_match() {
        let srv = make_service();
        let score = srv.score_turn_text("xyz", "你好");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_excerpt() {
        let result = ConversationRecallService::excerpt("这是一段很长的文本用来测试截取功能", 10);
        let char_count = result.chars().count();
        assert!(char_count <= 10, "字符数 {} 应 <= 10", char_count);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_compute_confidence() {
        let srv = make_service();
        // 空时间戳 → 满置信度
        assert_eq!(srv.compute_confidence(""), 1.0);
    }
}
