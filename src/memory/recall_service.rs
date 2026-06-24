use std::collections::HashSet;
use std::sync::Arc;

use crate::core::types::MemoryItem;
use crate::memory::stores::conversation::{
    ConversationRecord, ConversationTurnData, SessionRecord, SqliteConversationStore,
};
use crate::prelude::XueliResult;

/// 会话记忆回忆服务 — 从历史对话中检索与当前话题相关的轮次。
pub struct ConversationRecallService {
    store: Arc<SqliteConversationStore>,
    recent_session_limit: usize,
    recall_entry_limit: usize,
    min_match_score: f64,
    max_excerpt_chars: usize,
    recall_confidence_decay_per_day: f64,
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

/// 回忆到的单轮（内部辅助结构）
struct ScoredTurn {
    record: ConversationRecord,
    turn_user: String,
    turn_assistant: String,
    turn_timestamp: String,
    turn_id: usize,
    score: f64,
    recall_confidence: f64,
}

/// 匹配到的对话轮次（用于 build_recall_entries）
struct MatchedTurn {
    session: SessionRecord,
    turn: ConversationTurnData,
    score: f64,
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
        confidence.max(self.recall_confidence_minimum)
    }

    /// 构建话题召回条目（匹配 Python 版 build_recall_entries）
    pub async fn build_recall_entries(
        &self,
        user_id: &str,
        query: &str,
        message_type: &str,
        group_id: Option<&str>,
        dialogue_key: Option<&str>,
        platform: &str,
    ) -> XueliResult<Vec<RecallEntry>> {
        let normalized_query = Self::normalize_text(query);
        if normalized_query.len() < 2 {
            return Ok(vec![]);
        }

        let resolved_dialogue_key =
            self.store
                .build_dialogue_key(user_id, dialogue_key, message_type, group_id, platform);

        let is_group =
            message_type == "group" && group_id.map(|g| !g.trim().is_empty()).unwrap_or(false);

        let sessions = if is_group {
            self.store
                .get_conversations_by_group_id(
                    group_id.unwrap_or(""),
                    self.recent_session_limit.max(1),
                )
                .await?
        } else {
            self.store
                .get_conversations(user_id, self.recent_session_limit.max(1))
                .await?
        };

        let matched_sessions: Vec<SessionRecord> = sessions
            .into_iter()
            .filter(|s| s.dialogue_key == resolved_dialogue_key && s.turn_count() > 0)
            .collect();

        let mut matches: Vec<MatchedTurn> = Vec::new();
        for record in matched_sessions.into_iter().rev() {
            for turn in &record.turns {
                let score = self.score_turn(query, turn);
                if score < self.min_match_score {
                    continue;
                }
                let ts = if turn.timestamp.is_empty() {
                    &record.started_at
                } else {
                    &turn.timestamp
                };
                matches.push(MatchedTurn {
                    session: record.clone(),
                    turn: turn.clone(),
                    score,
                    recall_confidence: self.compute_confidence(ts),
                });
            }
        }

        if matches.is_empty() {
            return Ok(vec![]);
        }

        let first = &matches[0];
        let first_key = (first.session.session_id.clone(), first.turn.turn_id);
        let mut latest = first;
        let mut latest_key = first_key.clone();
        for m in &matches {
            let k = (m.session.session_id.clone(), m.turn.turn_id);
            if m.score > latest.score
                || (m.score == latest.score
                    && (m.turn.timestamp > latest.turn.timestamp
                        || (m.turn.timestamp == latest.turn.timestamp
                            && m.turn.turn_id > latest.turn.turn_id)))
            {
                latest = m;
                latest_key = k;
            }
        }

        let mut selected = vec![first];
        if first_key != latest_key {
            selected.push(latest);
        }

        let limit = self.recall_entry_limit.max(1);
        let entries: Vec<RecallEntry> = selected
            .into_iter()
            .take(limit)
            .enumerate()
            .map(|(idx, m)| {
                let label = if idx == 0 {
                    "第一次提到相关话题"
                } else {
                    "最近一次提到相关话题"
                };
                let content = self.format_entry(label, &m.session, &m.turn);
                RecallEntry {
                    content,
                    session_id: m.session.session_id.clone(),
                    dialogue_key: m.session.dialogue_key.clone(),
                    turn_id: m.turn.turn_id as usize,
                    score: m.score,
                    recall_confidence: m.recall_confidence,
                }
            })
            .collect();

        Ok(entries)
    }

    /// 格式化召回条目（匹配 Python _format_entry）
    fn format_entry(
        &self,
        label: &str,
        session: &SessionRecord,
        turn: &ConversationTurnData,
    ) -> String {
        let stamp = if turn.timestamp.is_empty() {
            if session.updated_at.is_empty() {
                &session.closed_at
            } else {
                &session.updated_at
            }
        } else {
            &turn.timestamp
        };
        let stamp = stamp.replace('T', " ");
        let stamp = &stamp[..stamp.len().min(16)];

        let prefix = if stamp.is_empty() {
            format!("{}（第{}轮）", label, turn.turn_id)
        } else {
            format!("{}（{}，第{}轮）", label, stamp, turn.turn_id)
        };

        let user_excerpt = Self::excerpt(&turn.user_message, self.max_excerpt_chars);
        let assistant_excerpt = Self::excerpt(&turn.assistant_message, self.max_excerpt_chars);

        let mut parts = vec![format!("{}：用户说“{}”", prefix, user_excerpt)];
        if !assistant_excerpt.is_empty() {
            parts.push(format!("你当时回复“{}”", assistant_excerpt));
        }
        parts.join("；")
    }

    /// 对查询和对话轮次进行匹配打分（匹配 Python _score_turn）
    fn score_turn(&self, query: &str, turn: &ConversationTurnData) -> f64 {
        let query_text = Self::normalize_text(query);
        if query_text.is_empty() {
            return 0.0;
        }
        let user_score = self.score_text(&query_text, &turn.user_message);
        let assistant_score = self.score_text(&query_text, &turn.assistant_message) * 0.35;
        user_score.max(user_score + assistant_score)
    }

    /// 计算规范化查询与文本的字符重合度得分（匹配 Python _score_text）
    fn score_text(&self, normalized_query: &str, text: &str) -> f64 {
        let normalized_text = Self::normalize_text(text);
        if normalized_query.is_empty() || normalized_text.is_empty() {
            return 0.0;
        }
        if normalized_text.contains(normalized_query) || normalized_query.contains(&normalized_text)
        {
            let shorter = normalized_query.len().min(normalized_text.len());
            let longer = normalized_query.len().max(normalized_text.len());
            shorter as f64 / longer.max(1) as f64
        } else {
            let q_chars: HashSet<char> = normalized_query.chars().collect();
            let t_chars: HashSet<char> = normalized_text.chars().collect();
            let overlap = q_chars.intersection(&t_chars).count();
            overlap as f64 / q_chars.len().max(1) as f64
        }
    }

    /// 召回与查询相关的历史记忆条目（保持向后兼容）
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

        let records = self
            .store
            .get_recent_by_scope("private", user_id, self.recent_session_limit)
            .await?;

        let mut matches: Vec<ScoredTurn> = Vec::new();
        for record in records.iter().rev() {
            let score = self.score_turn_text(query, &record.text);
            if score < self.min_match_score {
                continue;
            }
            let confidence = 1.0;
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
        if t.contains(&q) || q.contains(&t) {
            return q.len().min(t.len()) as f64 / q.len().max(t.len()) as f64;
        }
        let q_chars: HashSet<char> = q.chars().collect();
        let t_chars: HashSet<char> = t.chars().collect();
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

    /// 截取文本摘要（匹配 Python _excerpt：保留文字间空格，rstrip 后加省略号）
    fn excerpt(text: &str, max_chars: usize) -> String {
        let normalized = text.trim().split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.chars().count() <= max_chars {
            return normalized;
        }
        let truncated: String = normalized
            .chars()
            .take(max_chars.saturating_sub(3).max(1))
            .collect();
        format!("{}...", truncated.trim_end())
    }
}

impl Default for ConversationRecallService {
    fn default() -> Self {
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
    fn test_excerpt_preserves_spaces() {
        let result = ConversationRecallService::excerpt("Hello   World", 100);
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_compute_confidence() {
        let srv = make_service();
        assert_eq!(srv.compute_confidence(""), 1.0);
    }

    #[test]
    fn test_score_text_api() {
        let srv = make_service();
        let q = ConversationRecallService::normalize_text("咖啡");
        let score = srv.score_text(&q, "今天想喝咖啡");
        assert!(score > 0.0);
    }

    #[test]
    fn test_format_entry() {
        let srv = make_service();
        let session = SessionRecord {
            session_id: "sid1".to_string(),
            dialogue_key: "qq:private:u1".to_string(),
            user_id: "u1".to_string(),
            message_type: "private".to_string(),
            group_id: String::new(),
            started_at: "2024-01-01T00:00:00".to_string(),
            updated_at: "2024-01-01T00:01:00".to_string(),
            closed_at: String::new(),
            turns: vec![],
            metadata: std::collections::HashMap::new(),
            dirty_turns: 0,
            turn_count: 0,
        };
        let turn = ConversationTurnData {
            turn_id: 1,
            user_message: "你好".to_string(),
            assistant_message: "你好呀".to_string(),
            timestamp: "2024-01-01T00:00:00".to_string(),
            source_message_id: String::new(),
            source_group_id: String::new(),
            source_platform: String::new(),
            owner_user_id: "u1".to_string(),
            source_message_type: "private".to_string(),
            dialogue_key: "qq:private:u1".to_string(),
            image_description: String::new(),
        };
        let result = srv.format_entry("第一次提到相关话题", &session, &turn);
        assert!(result.contains("第一次提到相关话题"));
        assert!(result.contains("你好"));
        assert!(result.contains("你当时回复"));
    }
}
