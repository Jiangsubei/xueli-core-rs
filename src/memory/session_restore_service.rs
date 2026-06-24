use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::memory::extraction::chat_summary::ChatSummaryService;
use crate::memory::stores::conversation::{SessionRecord, SqliteConversationStore};
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
    ///
    /// 匹配 Python 版 build_restore_entries：从 conversation_sessions
    /// 获取已关闭会话，按 dialogue_key 过滤，取最近的 N 条。
    pub async fn build_restore_entries(
        &self,
        user_id: &str,
        scope_type: &str,
        scope_id: &str,
        platform: &str,
    ) -> XueliResult<Vec<SessionRestoreEntry>> {
        let resolved_dialogue_key = self.conversation_store.build_dialogue_key(
            user_id,
            None,
            scope_type,
            if scope_id.is_empty() {
                None
            } else {
                Some(scope_id)
            },
            platform,
        );

        let is_group = scope_type == "group" && !scope_id.is_empty();

        let sessions = if is_group {
            self.conversation_store
                .get_conversations_by_group_id(scope_id, self.recent_session_limit.max(1))
                .await?
        } else {
            self.conversation_store
                .get_conversations(user_id, self.recent_session_limit.max(1))
                .await?
        };

        let matched: Vec<SessionRecord> = sessions
            .into_iter()
            .filter(|s| {
                s.dialogue_key == resolved_dialogue_key
                    && !s.closed_at.trim().is_empty()
                    && s.turn_count() > 0
            })
            .collect();

        let mut entries: Vec<SessionRestoreEntry> = Vec::new();
        for (index, record) in matched
            .iter()
            .take(self.restore_entry_limit.max(1))
            .enumerate()
        {
            let messages = self
                .conversation_store
                .load_session(&record.session_id)
                .await?;
            let text_msgs: Vec<String> = messages.iter().map(|m| m.message_text.clone()).collect();

            let summary = self.summary_service.summarize(&text_msgs).await?;

            if summary.is_empty() {
                continue;
            }

            entries.push(SessionRestoreEntry {
                content: self.format_restore_entry(index + 1, record, &summary),
                metadata: SessionRestoreMeta {
                    session_id: record.session_id.clone(),
                    dialogue_key: record.dialogue_key.clone(),
                    closed_at: record.closed_at.clone(),
                    turn_count: record.turn_count(),
                },
            });
        }

        Ok(entries)
    }

    /// 格式化恢复条目文本（匹配 Python _format_restore_entry）
    fn format_restore_entry(&self, index: usize, record: &SessionRecord, summary: &str) -> String {
        let label = if index == 1 {
            "上一轮会话".to_string()
        } else {
            format!("更早一轮会话{}", index - 1)
        };
        let closed_at = if record.closed_at.is_empty() {
            record.updated_at.replace('T', " ")
        } else {
            record.closed_at.replace('T', " ")
        };
        let closed_at: String = closed_at.chars().take(16).collect();
        let suffix = if closed_at.is_empty() {
            format!("（{}轮）", record.turn_count())
        } else {
            format!("（{}，{}轮）", closed_at, record.turn_count())
        };
        format!("{}{}：{}", label, suffix, summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::stores::conversation::{ConversationTurnData, SqliteConversationStore};
    use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatCompletionResponse};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct StubAIClient;

    #[async_trait]
    impl AIClient for StubAIClient {
        async fn chat_completion(
            &self,
            _request: &ChatCompletionRequest,
        ) -> XueliResult<ChatCompletionResponse> {
            Ok(ChatCompletionResponse {
                content: "测试摘要内容".to_string(),
                segments: None,
                reasoning_content: String::new(),
                finish_reason: "stop".to_string(),
                usage: None,
                model: "test".to_string(),
                tool_calls: None,
                raw_content: String::new(),
                raw_response: None,
            })
        }
    }

    fn setup() -> (
        Arc<SqliteConversationStore>,
        tempfile::TempDir,
        SessionRestoreService<StubAIClient>,
    ) {
        let dir = tempfile::TempDir::new().expect("临时目录");
        let store = Arc::new(SqliteConversationStore::open(dir.path()).expect("打开数据库"));
        let client = StubAIClient;
        let summary = ChatSummaryService::new(Arc::new(client), "test-model");
        let svc = SessionRestoreService::new(store.clone(), summary, 6, 2);
        (store, dir, svc)
    }

    fn make_session_record(
        sid: &str,
        dk: &str,
        user: &str,
        closed: &str,
        turns: i64,
    ) -> SessionRecord {
        let now = chrono::Utc::now().to_rfc3339();
        let turn_data: Vec<ConversationTurnData> = (0..turns)
            .map(|i| ConversationTurnData {
                turn_id: i + 1,
                user_message: format!("用户消息{}", i),
                assistant_message: format!("回复{}", i),
                timestamp: now.clone(),
                source_message_id: String::new(),
                source_group_id: String::new(),
                source_platform: "qq".to_string(),
                owner_user_id: user.to_string(),
                source_message_type: "private".to_string(),
                dialogue_key: dk.to_string(),
                image_description: String::new(),
            })
            .collect();
        SessionRecord {
            session_id: sid.to_string(),
            dialogue_key: dk.to_string(),
            user_id: user.to_string(),
            message_type: "private".to_string(),
            group_id: String::new(),
            started_at: now.clone(),
            updated_at: now,
            closed_at: closed.to_string(),
            turns: turn_data,
            metadata: HashMap::new(),
            dirty_turns: 0,
            turn_count: 0,
        }
    }

    fn make_sid(user: &str, dk: &str, stamp: &str) -> String {
        format!(
            "session_{}_{}_{}_0000000a",
            user,
            dk.replace(':', "_"),
            stamp
        )
    }

    async fn insert_session(
        store: &SqliteConversationStore,
        sid: &str,
        dk: &str,
        user: &str,
        _closed: &str,
        turn_count: i64,
    ) {
        let stamp = if sid == "sid1" {
            "20240101000000"
        } else {
            "20240102000000"
        };
        let full_sid = make_sid(user, dk, stamp);
        let session = make_session_record(&full_sid, dk, user, "", turn_count);
        for turn in &session.turns {
            let user_msg = crate::memory::stores::conversation::MessageRecord::user(
                &turn.owner_user_id,
                &turn.owner_user_id,
                &turn.user_message,
                1000,
                &turn.source_message_id,
            );
            let assistant_msg = crate::memory::stores::conversation::MessageRecord::assistant(
                &turn.assistant_message,
                1001,
            );
            store
                .add_turn(&full_sid, &user_msg, &assistant_msg)
                .await
                .unwrap();
        }
        store.close_session(user, dk).await.unwrap();
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
        let session = make_session_record("sid1", "qq:private:u1", "u1", "2024-01-01T00:00:00", 3);
        let entry = svc.format_restore_entry(1, &session, "摘要内容");
        assert!(entry.contains("上一轮会话"));
        assert!(entry.contains("摘要内容"));
        assert!(entry.contains("3轮"));
    }

    #[tokio::test]
    async fn test_build_restore_entries() {
        let (store, _dir, svc) = setup();
        let dk = "qq:private:user1";
        insert_session(&store, "sid1", dk, "user1", "2024-01-01T00:00:00", 3).await;
        insert_session(&store, "sid2", dk, "user1", "2024-01-02T00:00:00", 2).await;

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
        let client = StubAIClient;
        let summary = ChatSummaryService::new(Arc::new(client), "test-model");
        let svc = SessionRestoreService::new(store.clone(), summary, 6, 2);

        let group_sid = "session_u1_qq_group_g123_20240101000000_abcdefgh";
        for i in 0..5 {
            let user_msg = crate::memory::stores::conversation::MessageRecord::user(
                &format!("u{}", i),
                &format!("User{}", i),
                &format!("群消息{}", i),
                1000 + i as i64,
                &format!("gm{}", i),
            );
            let assistant_msg = crate::memory::stores::conversation::MessageRecord::assistant(
                &format!("回复{}", i),
                1001,
            );
            store
                .add_turn(group_sid, &user_msg, &assistant_msg)
                .await
                .unwrap();
        }
        // Patch group_id, closed_at and message_type in DB since add_turn always creates private sessions
        let db_path = dir.path().join("conversations.db");
        let closed_at_str = chrono::Utc::now().to_rfc3339();
        {
            let db_path_clone = db_path.clone();
            let gid = "g123".to_string();
            let sid = group_sid.to_string();
            let closed = closed_at_str.clone();
            tokio::task::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&db_path_clone).unwrap();
                conn.execute(
                    "UPDATE conversation_sessions SET group_id = ?1, message_type = 'group', closed_at = ?2 WHERE session_id = ?3",
                    rusqlite::params![gid, closed, sid],
                ).unwrap();
            }).await.unwrap();
        }

        let entries = svc
            .build_restore_entries("u1", "group", "g123", "qq")
            .await
            .unwrap();

        assert!(!entries.is_empty());
    }
}
