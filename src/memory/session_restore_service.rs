use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::memory::extraction::chat_summary::ChatSummaryService;
use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};
use crate::prelude::XueliResult;
use crate::traits::ai_client::AIClient;

/// 会话恢复条目（用于提示词注入）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRestoreEntry {
    pub content: String,
    pub metadata: SessionRestoreMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRestoreMeta {
    pub session_id: String,
    pub dialogue_key: String,
    pub closed_at: String,
    pub turn_count: i64,
}

/// 会话恢复服务 — 加载最近同对话的会话摘要，用于提示词恢复。
///
/// 对应 Python 版 `src/memory/session_restore_service.py`
pub struct SessionRestoreService<A: AIClient> {
    conversation_store: Arc<SqliteConversationStore>,
    #[allow(dead_code)]
    summary_service: ChatSummaryService<A>,
    recent_session_limit: usize,
    restore_entry_limit: usize,
}

impl<A: AIClient> SessionRestoreService<A> {
    pub fn new(
        conversation_store: Arc<SqliteConversationStore>,
        summary_service: ChatSummaryService<A>,
        recent_session_limit: usize,
        restore_entry_limit: usize,
    ) -> Self {
        Self {
            conversation_store,
            summary_service,
            recent_session_limit: recent_session_limit.max(1),
            restore_entry_limit: restore_entry_limit.max(1),
        }
    }

    /// 构建对话键（格式：platform:scope_type:scope_id_or_user_id）
    pub fn build_dialogue_key(
        user_id: &str,
        scope_type: &str,
        scope_id: &str,
        platform: &str,
    ) -> String {
        if scope_type == "group" && !scope_id.is_empty() {
            format!("{}:{}:{}", platform, scope_type, scope_id)
        } else {
            format!("{}:private:{}", platform, user_id)
        }
    }

    /// 构建会话恢复条目列表
    pub async fn build_restore_entries(
        &self,
        user_id: &str,
        scope_type: &str,
        scope_id: &str,
        platform: &str,
    ) -> XueliResult<Vec<SessionRestoreEntry>> {
        let dialogue_key = Self::build_dialogue_key(user_id, scope_type, scope_id, platform);

        let is_group = scope_type == "group" && !scope_id.is_empty();

        // 获取最近消息，按会话分组
        let recent_messages = if is_group {
            self.conversation_store
                .get_recent_by_scope("group", scope_id, self.recent_session_limit * 20)
                .await
        } else {
            self.conversation_store
                .get_recent_by_user(user_id, self.recent_session_limit * 20)
                .await
        }
        .map_err(|e| format!("获取最近消息失败: {}", e))?;

        // 按 session_id 分组聚合
        let sessions = self.group_by_session(&recent_messages, &dialogue_key, user_id);

        let mut entries: Vec<SessionRestoreEntry> = Vec::new();
        let mut count = 0;

        for (session_id, messages) in &sessions {
            if count >= self.restore_entry_limit {
                break;
            }
            if messages.is_empty() {
                continue;
            }

            let turn_count = messages.len() as i64;
            let text_msgs: Vec<String> = messages.iter().map(|m| m.text.clone()).collect();
            let summary =
                ChatSummaryService::<crate::services::ai_client::DefaultAIClient>::summarize_simple(
                    &text_msgs,
                );

            let last_msg = messages.last().unwrap();
            // 使用最后一条消息时间作为 closed_at
            let closed_at = format!("{:.0}", last_msg.event_time);

            entries.push(SessionRestoreEntry {
                content: self.format_restore_entry(
                    count + 1,
                    session_id,
                    &closed_at,
                    turn_count,
                    &summary,
                ),
                metadata: SessionRestoreMeta {
                    session_id: session_id.clone(),
                    dialogue_key: dialogue_key.clone(),
                    closed_at,
                    turn_count,
                },
            });

            count += 1;
        }

        Ok(entries)
    }

    /// 按 session_id 分组消息，过滤出与目标对话键匹配的会话
    fn group_by_session(
        &self,
        messages: &[ConversationRecord],
        dialogue_key: &str,
        _user_id: &str,
    ) -> HashMap<String, Vec<ConversationRecord>> {
        let mut sessions: HashMap<String, Vec<ConversationRecord>> = HashMap::new();
        for msg in messages {
            let sid = if msg.session_id.is_empty() {
                dialogue_key.to_string()
            } else {
                msg.session_id.clone()
            };
            sessions.entry(sid).or_default().push(msg.clone());
        }
        // 按时间排序每组内的消息
        for msgs in sessions.values_mut() {
            msgs.sort_by(|a, b| {
                a.event_time
                    .partial_cmp(&b.event_time)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        sessions
    }

    /// 格式化恢复条目文本
    fn format_restore_entry(
        &self,
        index: usize,
        _session_id: &str,
        closed_at: &str,
        turn_count: i64,
        summary: &str,
    ) -> String {
        let label = if index == 1 {
            "上一轮会话"
        } else {
            "更早一轮会话"
        };
        let t = if !closed_at.is_empty() {
            format!("（{}，{}轮）", closed_at, turn_count)
        } else {
            format!("（{}轮）", turn_count)
        };
        format!("{}{}：{}", label, t, summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::stores::conversation::SqliteConversationStore;
    use std::sync::Arc;

    fn setup() -> (
        Arc<SqliteConversationStore>,
        tempfile::TempDir,
        SessionRestoreService<crate::services::ai_client::DefaultAIClient>,
    ) {
        let dir = tempfile::TempDir::new().expect("临时目录");
        let store = Arc::new(SqliteConversationStore::open(dir.path()).expect("打开数据库"));
        let client = crate::services::ai_client::DefaultAIClient::new(Arc::new(
            crate::core::config::ModelConfig {
                primary_model: "test-model".to_string(),
                light_model: "test-model".to_string(),
                ..Default::default()
            },
        ))
        .expect("创建 AI 客户端");
        let summary = ChatSummaryService::new(Arc::new(client), "test-model");
        let svc = SessionRestoreService::new(store.clone(), summary, 6, 2);
        (store, dir, svc)
    }

    async fn insert_msgs(store: &SqliteConversationStore, sid: &str, user_id: &str, count: usize) {
        for i in 0..count {
            let rec = ConversationRecord {
                id: 0,
                session_id: sid.to_string(),
                user_id: user_id.to_string(),
                sender_name: format!("User{}", user_id),
                text: format!("消息{}", i),
                is_bot: false,
                scope_type: "private".to_string(),
                scope_id: String::new(),
                event_time: 100.0 + i as f64,
                message_id: format!("m{}", i),
                platform: "qq".to_string(),
            };
            store.insert_message(&rec).await.unwrap();
        }
    }

    #[test]
    fn test_build_dialogue_key_private() {
        let key = SessionRestoreService::<crate::services::ai_client::DefaultAIClient>::build_dialogue_key(
            "user1", "private", "", "qq",
        );
        assert_eq!(key, "qq:private:user1");
    }

    #[test]
    fn test_build_dialogue_key_group() {
        let key = SessionRestoreService::<crate::services::ai_client::DefaultAIClient>::build_dialogue_key(
            "user1", "group", "g123", "qq",
        );
        assert_eq!(key, "qq:group:g123");
    }

    #[test]
    fn test_format_restore_entry() {
        let svc = setup().2;
        let entry = svc.format_restore_entry(1, "sid1", "100", 3, "摘要内容");
        assert!(entry.contains("上一轮会话"));
        assert!(entry.contains("摘要内容"));
        assert!(entry.contains("3轮"));
    }

    #[tokio::test]
    async fn test_build_restore_entries() {
        let (store, _dir, svc) = setup();
        insert_msgs(&store, "sid1", "user1", 3).await;
        insert_msgs(&store, "sid2", "user1", 2).await;

        let entries = svc
            .build_restore_entries("user1", "private", "", "qq")
            .await
            .unwrap();

        assert!(!entries.is_empty());
    }

    #[tokio::test]
    async fn test_build_restore_entries_empty() {
        let (_, _dir, svc) = setup();
        let entries = svc
            .build_restore_entries("unknown", "private", "", "qq")
            .await
            .unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_build_restore_entries_group() {
        let dir = tempfile::TempDir::new().expect("临时目录");
        let store = Arc::new(SqliteConversationStore::open(dir.path()).expect("打开数据库"));
        let client = crate::services::ai_client::DefaultAIClient::new(Arc::new(
            crate::core::config::ModelConfig {
                primary_model: "test-model".to_string(),
                light_model: "test-model".to_string(),
                ..Default::default()
            },
        ))
        .expect("创建 AI 客户端");
        let summary = ChatSummaryService::new(Arc::new(client), "test-model");
        let svc = SessionRestoreService::new(store.clone(), summary, 6, 2);

        for i in 0..5 {
            let rec = ConversationRecord {
                id: 0,
                session_id: "g123_session".to_string(),
                user_id: format!("u{}", i),
                sender_name: format!("User{}", i),
                text: format!("群消息{}", i),
                is_bot: false,
                scope_type: "group".to_string(),
                scope_id: "g123".to_string(),
                event_time: 100.0 + i as f64,
                message_id: format!("gm{}", i),
                platform: "qq".to_string(),
            };
            store.insert_message(&rec).await.unwrap();
        }

        let entries = svc
            .build_restore_entries("u1", "group", "g123", "qq")
            .await
            .unwrap();

        assert!(!entries.is_empty());
    }
}
