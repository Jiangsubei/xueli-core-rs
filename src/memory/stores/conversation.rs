use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio::sync::Semaphore;

use crate::prelude::XueliResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub id: i64,
    pub session_id: String,
    pub user_id: String,
    pub sender_name: String,
    pub text: String,
    pub is_bot: bool,
    pub scope_type: String,
    pub scope_id: String,
    pub event_time: f64,
    pub message_id: String,
    pub platform: String,
}

impl ConversationRecord {
    pub fn from_inbound(
        session_id: String,
        user_id: String,
        sender_name: String,
        text: String,
        scope_type: String,
        scope_id: String,
        event_time: f64,
        message_id: String,
        platform: String,
    ) -> Self {
        Self {
            id: 0,
            session_id,
            user_id,
            sender_name,
            text,
            is_bot: false,
            scope_type,
            scope_id,
            event_time,
            message_id,
            platform,
        }
    }

    pub fn from_bot_reply(
        session_id: String,
        user_id: String,
        text: String,
        scope_type: String,
        scope_id: String,
        event_time: f64,
    ) -> Self {
        Self {
            id: 0,
            session_id,
            user_id,
            sender_name: "bot".to_string(),
            text,
            is_bot: true,
            scope_type,
            scope_id,
            event_time,
            message_id: String::new(),
            platform: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub user_id: String,
    pub display_name: String,
    pub message_text: String,
    pub raw_text: String,
    pub image_descriptions: Vec<String>,
    pub message_kind: String,
    pub segments: serde_json::Value,
    pub timestamp: i64,
    pub message_id: String,
    pub speaker_role: String,
}

impl MessageRecord {
    pub fn user(
        user_id: impl Into<String>,
        display_name: impl Into<String>,
        text: impl Into<String>,
        timestamp: i64,
        message_id: impl Into<String>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            display_name: display_name.into(),
            message_text: text.into(),
            raw_text: String::new(),
            image_descriptions: Vec::new(),
            message_kind: "text".to_string(),
            segments: serde_json::Value::Array(Vec::new()),
            timestamp,
            message_id: message_id.into(),
            speaker_role: "user".to_string(),
        }
    }

    pub fn assistant(text: impl Into<String>, timestamp: i64) -> Self {
        Self {
            user_id: String::new(),
            display_name: "bot".to_string(),
            message_text: text.into(),
            raw_text: String::new(),
            image_descriptions: Vec::new(),
            message_kind: "text".to_string(),
            segments: serde_json::Value::Array(Vec::new()),
            timestamp,
            message_id: String::new(),
            speaker_role: "assistant".to_string(),
        }
    }
}

/// add_turn 返回的注册信息 — 对应 Python 版 ConversationTurnRegistration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurnRegistration {
    pub session_id: String,
    pub turn_id: i64,
    pub turn_count: i64,
    pub user_id: String,
    pub dialogue_key: String,
    #[serde(default)]
    pub closed_session_id: String,
    #[serde(default)]
    pub closed_session_user_id: String,
}

/// 内存中的会话 turn 数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurnData {
    pub turn_id: i64,
    pub user_message: String,
    pub assistant_message: String,
    pub timestamp: String,
    pub source_message_id: String,
    pub source_group_id: String,
    pub source_platform: String,
    pub owner_user_id: String,
    pub source_message_type: String,
    pub dialogue_key: String,
    pub image_description: String,
}

/// 内存中的会话记录 — 对应 Python 版 ConversationRecord
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub dialogue_key: String,
    pub user_id: String,
    pub message_type: String,
    pub group_id: String,
    pub started_at: String,
    pub updated_at: String,
    pub closed_at: String,
    pub turns: Vec<ConversationTurnData>,
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    pub dirty_turns: i64,
}

impl SessionRecord {
    pub fn turn_count(&self) -> i64 {
        self.turns.len() as i64
    }
}

pub struct SqliteConversationStore {
    conn: Mutex<Connection>,
    db_path: PathBuf,
    _write_sem: Semaphore,
    /// 内存中的活跃会话缓存（session_id → SessionRecord）
    sessions: Mutex<HashMap<String, SessionRecord>>,
    /// dialogue_key → 活跃 session_id 的映射
    active_session_by_dialogue: Mutex<HashMap<String, String>>,
    /// 会话超时秒数
    session_timeout_seconds: u64,
}

const INIT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS conversation_messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL,
    user_id         TEXT NOT NULL,
    sender_name     TEXT NOT NULL,
    text            TEXT NOT NULL,
    is_bot          INTEGER DEFAULT 0,
    scope_type      TEXT NOT NULL,
    scope_id        TEXT DEFAULT '',
    event_time      REAL NOT NULL,
    message_id      TEXT DEFAULT '',
    platform        TEXT DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_cm_session_time
    ON conversation_messages(session_id, event_time DESC);

CREATE INDEX IF NOT EXISTS idx_cm_scope_time
    ON conversation_messages(scope_type, scope_id, event_time DESC);

CREATE TABLE IF NOT EXISTS conversation_sessions (
    session_id      TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL,
    scope_key       TEXT NOT NULL,
    message_type    TEXT DEFAULT 'private',
    group_id        TEXT DEFAULT '',
    platform        TEXT DEFAULT '',
    created_at      TEXT NOT NULL,
    closed_at       TEXT DEFAULT '',
    turn_count      INTEGER DEFAULT 0,
    metadata        TEXT DEFAULT '{}',
    dialogue_key    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cs_dialogue
    ON conversation_sessions(dialogue_key);
CREATE INDEX IF NOT EXISTS idx_cs_user
    ON conversation_sessions(user_id);

CREATE TABLE IF NOT EXISTS conversation_turns (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id             INTEGER NOT NULL,
    session_id          TEXT NOT NULL,
    user_msg_json       TEXT NOT NULL,
    assistant_msg_json  TEXT NOT NULL,
    timestamp           INTEGER NOT NULL,
    source_message_id   TEXT DEFAULT '',
    UNIQUE(session_id, turn_id)
);

CREATE INDEX IF NOT EXISTS idx_ct_session
    ON conversation_turns(session_id);

CREATE TABLE IF NOT EXISTS group_messages (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id          TEXT DEFAULT '',
    group_id            TEXT NOT NULL,
    user_id             TEXT DEFAULT '',
    display_name        TEXT DEFAULT '',
    message_text        TEXT NOT NULL,
    raw_text            TEXT NOT NULL,
    image_descriptions  TEXT DEFAULT '[]',
    message_kind        TEXT DEFAULT 'text',
    segments            TEXT DEFAULT '[]',
    timestamp           INTEGER NOT NULL,
    message_id          TEXT DEFAULT '0',
    speaker_role        TEXT DEFAULT 'user'
);

CREATE INDEX IF NOT EXISTS idx_gm_group_time
    ON group_messages(group_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_gm_message_id
    ON group_messages(message_id);

CREATE TABLE IF NOT EXISTS private_messages (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id          TEXT DEFAULT '',
    user_id             TEXT DEFAULT '',
    display_name        TEXT DEFAULT '',
    message_text        TEXT NOT NULL,
    raw_text            TEXT NOT NULL,
    image_descriptions  TEXT DEFAULT '[]',
    message_kind        TEXT DEFAULT 'text',
    segments            TEXT DEFAULT '[]',
    timestamp           INTEGER NOT NULL,
    message_id          TEXT DEFAULT '0',
    speaker_role        TEXT DEFAULT 'user'
);

CREATE INDEX IF NOT EXISTS idx_pm_user_time
    ON private_messages(user_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_pm_message_id
    ON private_messages(message_id);
"#;

impl SqliteConversationStore {
    pub fn open(db_dir: &Path) -> XueliResult<Self> {
        Self::open_with_timeout(db_dir, 3600)
    }

    pub fn open_with_timeout(db_dir: &Path, session_timeout_seconds: u64) -> XueliResult<Self> {
        std::fs::create_dir_all(db_dir).map_err(|e| format!("无法创建目录: {e}"))?;
        let db_path = db_dir.join("conversations.db");

        let conn =
            Connection::open(&db_path).map_err(|e| format!("无法打开数据库 {db_path:?}: {e}"))?;

        conn.execute_batch("PRAGMA journal_mode=WAL")
            .map_err(|e| format!("PRAGMA 失败: {e}"))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL")
            .map_err(|e| format!("PRAGMA 失败: {e}"))?;
        conn.execute_batch(INIT_SCHEMA)
            .map_err(|e| format!("建表失败: {e}"))?;

        Ok(Self {
            conn: Mutex::new(conn),
            db_path,
            _write_sem: Semaphore::new(5),
            sessions: Mutex::new(HashMap::new()),
            active_session_by_dialogue: Mutex::new(HashMap::new()),
            session_timeout_seconds: session_timeout_seconds.max(1),
        })
    }

    pub fn insert_message(&self, record: &ConversationRecord) -> XueliResult<i64> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        conn.execute(
            "INSERT INTO conversation_messages
             (session_id, user_id, sender_name, text, is_bot, scope_type, scope_id, event_time, message_id, platform)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                record.session_id,
                record.user_id,
                record.sender_name,
                record.text,
                record.is_bot as i32,
                record.scope_type,
                record.scope_id,
                record.event_time,
                record.message_id,
                record.platform,
            ],
        )
        .map_err(|e| format!("插入失败: {e}"))?;

        Ok(conn.last_insert_rowid())
    }

    pub fn insert_messages(&self, records: &[ConversationRecord]) -> XueliResult<usize> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("事务失败: {e}"))?;

        for record in records {
            tx.execute(
                "INSERT INTO conversation_messages
                 (session_id, user_id, sender_name, text, is_bot, scope_type, scope_id, event_time, message_id, platform)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    record.session_id,
                    record.user_id,
                    record.sender_name,
                    record.text,
                    record.is_bot as i32,
                    record.scope_type,
                    record.scope_id,
                    record.event_time,
                    record.message_id,
                    record.platform,
                ],
            )
            .map_err(|e| format!("批量插入失败: {e}"))?;
        }

        tx.commit().map_err(|e| format!("提交事务失败: {e}"))?;
        Ok(records.len())
    }

    pub fn get_recent_by_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> XueliResult<Vec<ConversationRecord>> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, user_id, sender_name, text, is_bot,
                        scope_type, scope_id, event_time, message_id, platform
                 FROM conversation_messages
                 WHERE session_id = ?1
                 ORDER BY event_time DESC
                 LIMIT ?2",
            )
            .map_err(|e| format!("准备查询失败: {e}"))?;

        let rows = stmt
            .query_map(params![session_id, limit as i64], |row| {
                Ok(ConversationRecord {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    user_id: row.get(2)?,
                    sender_name: row.get(3)?,
                    text: row.get(4)?,
                    is_bot: row.get::<_, i32>(5)? != 0,
                    scope_type: row.get(6)?,
                    scope_id: row.get(7)?,
                    event_time: row.get(8)?,
                    message_id: row.get(9)?,
                    platform: row.get(10)?,
                })
            })
            .map_err(|e| format!("查询失败: {e}"))?;

        let mut records: Vec<ConversationRecord> = Vec::new();
        for row in rows {
            records.push(row.map_err(|e| format!("读取行失败: {e}"))?);
        }
        records.reverse();
        Ok(records)
    }

    pub fn get_recent_by_scope(
        &self,
        scope_type: &str,
        scope_id: &str,
        limit: usize,
    ) -> XueliResult<Vec<ConversationRecord>> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, user_id, sender_name, text, is_bot,
                        scope_type, scope_id, event_time, message_id, platform
                 FROM conversation_messages
                 WHERE scope_type = ?1 AND scope_id = ?2
                 ORDER BY event_time DESC
                 LIMIT ?3",
            )
            .map_err(|e| format!("准备查询失败: {e}"))?;

        let rows = stmt
            .query_map(params![scope_type, scope_id, limit as i64], |row| {
                Ok(ConversationRecord {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    user_id: row.get(2)?,
                    sender_name: row.get(3)?,
                    text: row.get(4)?,
                    is_bot: row.get::<_, i32>(5)? != 0,
                    scope_type: row.get(6)?,
                    scope_id: row.get(7)?,
                    event_time: row.get(8)?,
                    message_id: row.get(9)?,
                    platform: row.get(10)?,
                })
            })
            .map_err(|e| format!("查询失败: {e}"))?;

        let mut records: Vec<ConversationRecord> = Vec::new();
        for row in rows {
            records.push(row.map_err(|e| format!("读取行失败: {e}"))?);
        }
        records.reverse();
        Ok(records)
    }

    pub fn get_recent_by_user(
        &self,
        user_id: &str,
        limit: usize,
    ) -> XueliResult<Vec<ConversationRecord>> {
        let conn = self.conn.lock().map_err(|e| format!("锁错误: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, user_id, sender_name, text, is_bot,
                        scope_type, scope_id, event_time, message_id, platform
                 FROM conversation_messages
                 WHERE user_id = ?1
                 ORDER BY event_time DESC
                 LIMIT ?2",
            )
            .map_err(|e| format!("准备查询失败: {e}"))?;

        let rows = stmt
            .query_map(params![user_id, limit as i64], |row| {
                Ok(ConversationRecord {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    user_id: row.get(2)?,
                    sender_name: row.get(3)?,
                    text: row.get(4)?,
                    is_bot: row.get::<_, i32>(5)? != 0,
                    scope_type: row.get(6)?,
                    scope_id: row.get(7)?,
                    event_time: row.get(8)?,
                    message_id: row.get(9)?,
                    platform: row.get(10)?,
                })
            })
            .map_err(|e| format!("查询失败: {e}"))?;

        let mut records: Vec<ConversationRecord> = Vec::new();
        for row in rows {
            records.push(row.map_err(|e| format!("读取行失败: {e}"))?);
        }
        records.reverse();
        Ok(records)
    }

    pub async fn active_session_ids(&self) -> XueliResult<Vec<String>> {
        let db_path = self.db_path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let mut stmt = conn
                .prepare(
                    "SELECT session_id FROM conversation_sessions WHERE closed_at = '' \
                     ORDER BY created_at DESC",
                )
                .map_err(|e| format!("查询失败: {e}"))?;
            let ids = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| format!("查询失败: {e}"))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(ids)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    pub async fn add_turn(
        &self,
        session_id: &str,
        user_msg: &MessageRecord,
        assistant_msg: &MessageRecord,
    ) -> XueliResult<ConversationTurnRegistration> {
        let session_id_owned = session_id.to_string();
        let user_id = user_msg.user_id.clone();
        let user_json = serde_json::to_string(user_msg).map_err(|e| format!("序列化失败: {e}"))?;
        let assistant_json =
            serde_json::to_string(assistant_msg).map_err(|e| format!("序列化失败: {e}"))?;
        let timestamp = user_msg.timestamp;
        let source_msg_id = user_msg.message_id.clone();
        let db_path = self.db_path.clone();

        // 更新内存缓存
        let (turn_id, turn_count, dialogue_key) = {
            let mut sessions = self.sessions.lock().map_err(|e| format!("锁错误: {e}"))?;
            let mut by_dialogue = self
                .active_session_by_dialogue
                .lock()
                .map_err(|e| format!("锁错误: {e}"))?;

            let now = chrono::Utc::now().to_rfc3339();
            let resolved_dk = Self::parse_session_id(&session_id_owned)
                .map(|(_, dk, _, _)| dk)
                .unwrap_or_else(|| session_id_owned.clone());

            let session = sessions.entry(session_id_owned.clone()).or_insert_with(|| {
                by_dialogue.insert(resolved_dk.clone(), session_id_owned.clone());
                SessionRecord {
                    session_id: session_id_owned.clone(),
                    dialogue_key: resolved_dk,
                    user_id: user_id.clone(),
                    message_type: "private".to_string(),
                    group_id: String::new(),
                    started_at: now.clone(),
                    updated_at: now.clone(),
                    closed_at: String::new(),
                    turns: Vec::new(),
                    metadata: HashMap::new(),
                    dirty_turns: 0,
                }
            });

            let tid = session.turns.len() as i64 + 1;
            let dk = session.dialogue_key.clone();
            session.turns.push(ConversationTurnData {
                turn_id: tid,
                user_message: user_msg.message_text.clone(),
                assistant_message: assistant_msg.message_text.clone(),
                timestamp: now.clone(),
                source_message_id: source_msg_id.clone(),
                source_group_id: String::new(),
                source_platform: String::new(),
                owner_user_id: user_id.clone(),
                source_message_type: "private".to_string(),
                dialogue_key: dk.clone(),
                image_description: String::new(),
            });
            session.updated_at = now;
            session.metadata.insert(
                "latest_message_id".to_string(),
                source_msg_id.clone(),
            );
            session.dirty_turns += 1;

            (tid, session.turn_count(), dk)
        };

        // 持久化到 SQLite
        let sid_for_db = session_id_owned.clone();
        let user_id_for_db = user_id.clone();
        let user_id_for_result = user_id.clone();
        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;

            let now = chrono::Utc::now().to_rfc3339();
            let (scope_key, dk) = Self::parse_session_id(&sid_for_db)
                .map(|(_, dk, _, _)| (dk.clone(), dk))
                .unwrap_or((sid_for_db.clone(), sid_for_db.clone()));
            conn.execute(
                "INSERT OR IGNORE INTO conversation_sessions
                 (session_id, user_id, scope_key, message_type, group_id, platform, created_at, closed_at, turn_count, metadata, dialogue_key)
                 VALUES (?1, ?2, ?3, 'private', '', '', ?4, '', 0, '{}', ?5)",
                params![sid_for_db, user_id_for_db, scope_key, now, dk],
            )
            .map_err(|e| format!("创建会话失败: {e}"))?;

            conn.execute(
                "INSERT OR REPLACE INTO conversation_turns
                 (turn_id, session_id, user_msg_json, assistant_msg_json, timestamp, source_message_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![turn_id, sid_for_db, user_json, assistant_json, timestamp, source_msg_id],
            )
            .map_err(|e| format!("插入 turn 失败: {e}"))?;

            conn.execute(
                "UPDATE conversation_sessions SET turn_count = ?1 WHERE session_id = ?2",
                params![turn_id, sid_for_db],
            )
            .map_err(|e| format!("更新 turn_count 失败: {e}"))?;

            Ok::<(), String>(())
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))??;

        Ok(ConversationTurnRegistration {
            session_id: session_id_owned,
            turn_id,
            turn_count,
            user_id: user_id_for_result,
            dialogue_key,
            closed_session_id: String::new(),
            closed_session_user_id: String::new(),
        })
    }

    /// 注册对话轮次 — 对应 Python 版 add_turn，自动解析会话
    pub fn register_turn(
        &self,
        user_id: &str,
        user_message: &str,
        assistant_message: &str,
        dialogue_key: Option<&str>,
        message_type: &str,
        group_id: Option<&str>,
        message_id: Option<&str>,
        image_description: &str,
        source_platform: &str,
    ) -> ConversationTurnRegistration {
        let resolved_dk = self.build_dialogue_key(
            user_id,
            dialogue_key,
            message_type,
            group_id,
            source_platform,
        );

        let (session_id, closed_session_id) = self.ensure_session(
            user_id,
            &resolved_dk,
            message_type,
            group_id.unwrap_or(""),
        );

        let now = chrono::Utc::now().to_rfc3339();
        let (turn_id, turn_count, sid, dk) = {
            let mut sessions = self
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let session = sessions.get_mut(&session_id).expect("session must exist after ensure");

            let tid = session.turns.len() as i64 + 1;
            session.turns.push(ConversationTurnData {
                turn_id: tid,
                user_message: user_message.to_string(),
                assistant_message: assistant_message.to_string(),
                timestamp: now.clone(),
                source_message_id: message_id.unwrap_or("").to_string(),
                source_group_id: group_id.unwrap_or("").to_string(),
                source_platform: source_platform.to_string(),
                owner_user_id: user_id.to_string(),
                source_message_type: message_type.to_string(),
                dialogue_key: resolved_dk.clone(),
                image_description: image_description.to_string(),
            });
            session.updated_at = now;
            session.message_type = message_type.to_string();
            session.group_id = group_id.unwrap_or("").to_string();
            session
                .metadata
                .insert("latest_message_id".to_string(), message_id.unwrap_or("").to_string());
            session.dirty_turns += 1;

            (tid, session.turn_count(), session.session_id.clone(), session.dialogue_key.clone())
        };

        let closed_user_id = if !closed_session_id.is_empty() {
            let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions
                .get(&closed_session_id)
                .map(|s| s.user_id.clone())
                .unwrap_or_else(|| user_id.to_string())
        } else {
            String::new()
        };

        ConversationTurnRegistration {
            session_id: sid,
            turn_id,
            turn_count,
            user_id: user_id.to_string(),
            dialogue_key: dk,
            closed_session_id,
            closed_session_user_id: closed_user_id,
        }
    }

    /// 持久化内存中指定 session 的所有 turns 到 SQLite
    pub async fn persist_session(&self, session_id: &str) -> XueliResult<bool> {
        let session_data = {
            let sessions = self.sessions.lock().map_err(|e| format!("锁错误: {e}"))?;
            sessions.get(session_id).cloned()
        };

        let session = match session_data {
            Some(s) => s,
            None => return Ok(false),
        };

        if session.turns.is_empty() {
            return Ok(false);
        }

        let db_path = self.db_path.clone();
        let sid = session_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let now = chrono::Utc::now().to_rfc3339();
            let closed_at = if session.closed_at.is_empty() {
                now.clone()
            } else {
                session.closed_at.clone()
            };

            // 合并已有 metadata
            let existing_meta: String = conn
                .query_row(
                    "SELECT metadata FROM conversation_sessions WHERE session_id = ?1",
                    params![sid],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| "{}".to_string());

            let mut merged_meta: serde_json::Value =
                serde_json::from_str(&existing_meta).unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
            if let serde_json::Value::Object(ref mut map) = merged_meta {
                for (k, v) in &session.metadata {
                    map.insert(k.clone(), serde_json::Value::String(v.clone()));
                }
            }
            let merged_json = serde_json::to_string(&merged_meta).unwrap_or_else(|_| "{}".to_string());

            conn.execute(
                "INSERT OR REPLACE INTO conversation_sessions
                 (session_id, user_id, scope_key, message_type, group_id, platform, created_at, closed_at, turn_count, metadata, dialogue_key)
                 VALUES (?1, ?2, ?3, ?4, ?5, '', ?6, ?7, ?8, ?9, ?10)",
                params![
                    sid,
                    session.user_id,
                    session.dialogue_key,
                    session.message_type,
                    session.group_id,
                    session.started_at,
                    closed_at,
                    session.turn_count(),
                    merged_json,
                    session.dialogue_key,
                ],
            )
            .map_err(|e| format!("写入会话失败: {e}"))?;

            for turn in &session.turns {
                let user_msg = MessageRecord::user(
                    &turn.owner_user_id,
                    &turn.owner_user_id,
                    &turn.user_message,
                    0,
                    &turn.source_message_id,
                );
                let assistant_msg = MessageRecord::assistant(&turn.assistant_message, 0);
                let user_json = serde_json::to_string(&user_msg).unwrap_or_default();
                let assistant_json = serde_json::to_string(&assistant_msg).unwrap_or_default();

                conn.execute(
                    "INSERT OR REPLACE INTO conversation_turns
                     (turn_id, session_id, user_msg_json, assistant_msg_json, timestamp, source_message_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![turn.turn_id, sid, user_json, assistant_json, turn.timestamp, turn.source_message_id],
                )
                .map_err(|e| format!("写入 turn 失败: {e}"))?;
            }

            Ok(true)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    pub async fn close_session(
        &self,
        user_id: &str,
        scope_key: &str,
    ) -> XueliResult<Option<String>> {
        // 清除内存缓存
        {
            let mut by_dialogue = self
                .active_session_by_dialogue
                .lock()
                .map_err(|e| format!("锁错误: {e}"))?;
            by_dialogue.remove(scope_key);
        }

        let user_id = user_id.to_string();
        let scope_key = scope_key.to_string();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let now = chrono::Utc::now().to_rfc3339();

            let affected = conn
                .execute(
                    "UPDATE conversation_sessions SET closed_at = ?1 \
                     WHERE closed_at = '' AND user_id = ?2 AND scope_key = ?3",
                    params![now, user_id, scope_key],
                )
                .map_err(|e| format!("关闭会话失败: {e}"))?;

            if affected == 0 {
                return Ok(None);
            }

            let session_id: Option<String> = conn
                .query_row(
                    "SELECT session_id FROM conversation_sessions \
                     WHERE closed_at = ?1 AND user_id = ?2 AND scope_key = ?3",
                    params![now, user_id, scope_key],
                    |row| row.get(0),
                )
                .ok();

            Ok(session_id)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    pub async fn close_all_sessions(&self) -> XueliResult<Vec<String>> {
        // 清除内存缓存
        {
            let mut by_dialogue = self
                .active_session_by_dialogue
                .lock()
                .map_err(|e| format!("锁错误: {e}"))?;
            by_dialogue.clear();
        }

        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let now = chrono::Utc::now().to_rfc3339();

            let ids: Vec<String> = {
                let mut stmt = conn
                    .prepare("SELECT session_id FROM conversation_sessions WHERE closed_at = ''")
                    .map_err(|e| format!("查询失败: {e}"))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| format!("查询失败: {e}"))?;
                rows.filter_map(|r| r.ok()).collect()
            };

            conn.execute(
                "UPDATE conversation_sessions SET closed_at = ?1 WHERE closed_at = ''",
                params![now],
            )
            .map_err(|e| format!("关闭所有会话失败: {e}"))?;

            Ok(ids)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    pub async fn save_conversation(
        &self,
        session_id: &str,
        messages: &[MessageRecord],
    ) -> XueliResult<()> {
        let session_id = session_id.to_string();
        let messages_vec = messages.to_vec();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;

            let first_user_id = messages_vec
                .iter()
                .find(|m| m.speaker_role == "user")
                .map(|m| m.user_id.as_str())
                .unwrap_or("");
            let now = chrono::Utc::now().to_rfc3339();
            let (scope_key, dialogue_key) = SqliteConversationStore::parse_session_id(&session_id)
                .map(|(_, dk, _, _)| (dk.clone(), dk))
                .unwrap_or((session_id.clone(), session_id.clone()));
            conn.execute(
                "INSERT OR IGNORE INTO conversation_sessions
                 (session_id, user_id, scope_key, message_type, group_id, platform, created_at, closed_at, turn_count, metadata, dialogue_key)
                 VALUES (?1, ?2, ?3, 'private', '', '', ?4, '', 0, '{}', ?5)",
                params![session_id, first_user_id, scope_key, now, dialogue_key],
            )
            .map_err(|e| format!("创建会话失败: {e}"))?;

            let existing_count: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(turn_id), 0) FROM conversation_turns WHERE session_id = ?1",
                    params![session_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            let mut turn_id = existing_count + 1;
            let mut i = 0;
            while i + 1 < messages_vec.len() {
                let user_msg = &messages_vec[i];
                let assistant_msg = &messages_vec[i + 1];
                let user_json = serde_json::to_string(user_msg).unwrap_or_default();
                let assistant_json = serde_json::to_string(assistant_msg).unwrap_or_default();
                let source_msg_id = user_msg.message_id.clone();
                let timestamp = user_msg.timestamp;

                let _ = conn.execute(
                    "INSERT OR REPLACE INTO conversation_turns
                     (turn_id, session_id, user_msg_json, assistant_msg_json, timestamp, source_message_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![turn_id, session_id, user_json, assistant_json, timestamp, source_msg_id],
                );

                turn_id += 1;
                i += 2;
            }

            let _ = conn.execute(
                "UPDATE conversation_sessions SET turn_count = ?1 WHERE session_id = ?2",
                params![turn_id - 1, session_id],
            );

            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    pub async fn load_session(&self, session_id: &str) -> XueliResult<Vec<MessageRecord>> {
        let session_id = session_id.to_string();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let mut stmt = conn
                .prepare(
                    "SELECT user_msg_json, assistant_msg_json FROM conversation_turns \
                     WHERE session_id = ?1 ORDER BY turn_id ASC",
                )
                .map_err(|e| format!("查询失败: {e}"))?;

            let pairs: Vec<(String, String)> = stmt
                .query_map(params![session_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("查询失败: {e}"))?
                .filter_map(|r| r.ok())
                .collect();

            let mut messages: Vec<MessageRecord> = Vec::new();
            for (user_json, assistant_json) in pairs {
                if let Ok(msg) = serde_json::from_str::<MessageRecord>(&user_json) {
                    messages.push(msg);
                }
                if let Ok(msg) = serde_json::from_str::<MessageRecord>(&assistant_json) {
                    messages.push(msg);
                }
            }
            Ok(messages)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    pub async fn update_session_metadata(
        &self,
        session_id: &str,
        metadata: &HashMap<String, String>,
    ) -> XueliResult<()> {
        let session_id = session_id.to_string();
        let metadata = metadata.clone();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;

            let existing_json: String = conn
                .query_row(
                    "SELECT metadata FROM conversation_sessions WHERE session_id = ?1",
                    params![session_id],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| "{}".to_string());

            let mut merged: serde_json::Value = serde_json::from_str(&existing_json)
                .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
            if let serde_json::Value::Object(ref mut map) = merged {
                for (k, v) in &metadata {
                    map.insert(k.clone(), serde_json::Value::String(v.clone()));
                }
            }
            let merged_json = serde_json::to_string(&merged).unwrap_or_else(|_| "{}".to_string());

            conn.execute(
                "UPDATE conversation_sessions SET metadata = ?1 WHERE session_id = ?2",
                params![merged_json, session_id],
            )
            .map_err(|e| format!("更新元数据失败: {e}"))?;

            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    pub async fn add_group_message(&self, group_id: &str, msg: &MessageRecord) -> XueliResult<()> {
        let group_id = group_id.to_string();
        let msg = msg.clone();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let image_json =
                serde_json::to_string(&msg.image_descriptions).unwrap_or_else(|_| "[]".to_string());
            let segments_json =
                serde_json::to_string(&msg.segments).unwrap_or_else(|_| "[]".to_string());

            conn.execute(
                "INSERT INTO group_messages
                 (session_id, group_id, user_id, display_name, message_text, raw_text,
                  image_descriptions, message_kind, segments, timestamp, message_id, speaker_role)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    "",
                    group_id,
                    msg.user_id,
                    msg.display_name,
                    msg.message_text,
                    msg.raw_text,
                    image_json,
                    msg.message_kind,
                    segments_json,
                    msg.timestamp,
                    msg.message_id,
                    msg.speaker_role,
                ],
            )
            .map_err(|e| format!("插入群聊消息失败: {e}"))?;

            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    pub async fn add_private_message(&self, user_id: &str, msg: &MessageRecord) -> XueliResult<()> {
        let user_id = user_id.to_string();
        let msg = msg.clone();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let image_json =
                serde_json::to_string(&msg.image_descriptions).unwrap_or_else(|_| "[]".to_string());
            let segments_json =
                serde_json::to_string(&msg.segments).unwrap_or_else(|_| "[]".to_string());

            conn.execute(
                "INSERT INTO private_messages
                 (session_id, user_id, display_name, message_text, raw_text, image_descriptions,
                  message_kind, segments, timestamp, message_id, speaker_role)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    "",
                    user_id,
                    msg.display_name,
                    msg.message_text,
                    msg.raw_text,
                    image_json,
                    msg.message_kind,
                    segments_json,
                    msg.timestamp,
                    msg.message_id,
                    msg.speaker_role,
                ],
            )
            .map_err(|e| format!("插入私聊消息失败: {e}"))?;

            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    pub async fn get_messages_after_id(
        &self,
        group_id: &str,
        after_id: &str,
    ) -> XueliResult<Vec<MessageRecord>> {
        let group_id = group_id.to_string();
        let after_id = after_id.to_string();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let after_rowid: i64 = conn
                .query_row(
                    "SELECT COALESCE(id, 0) FROM group_messages WHERE message_id = ?1 LIMIT 1",
                    params![after_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            let mut stmt = conn
                .prepare(
                    "SELECT user_id, display_name, message_text, raw_text, image_descriptions,
                            message_kind, segments, timestamp, message_id, speaker_role
                     FROM group_messages
                     WHERE group_id = ?1 AND id > ?2
                     ORDER BY timestamp ASC, id ASC",
                )
                .map_err(|e| format!("查询失败: {e}"))?;

            let records: Vec<MessageRecord> = stmt
                .query_map(params![group_id, after_rowid], |row| {
                    let img_str: String = row.get(4)?;
                    let seg_str: String = row.get(6)?;
                    Ok(MessageRecord {
                        user_id: row.get(0)?,
                        display_name: row.get(1)?,
                        message_text: row.get(2)?,
                        raw_text: row.get(3)?,
                        image_descriptions: serde_json::from_str(&img_str).unwrap_or_default(),
                        message_kind: row.get(5)?,
                        segments: serde_json::from_str(&seg_str).unwrap_or_default(),
                        timestamp: row.get(7)?,
                        message_id: row.get(8)?,
                        speaker_role: row.get(9)?,
                    })
                })
                .map_err(|e| format!("查询失败: {e}"))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(records)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    pub fn generate_session_id(&self, user_id: &str, dialogue_key: &str) -> String {
        let normalized = dialogue_key.replace(':', "_");
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let suffix = uuid::Uuid::new_v4().to_string().replace('-', "")[..8].to_string();
        format!("session_{user_id}_{normalized}_{stamp}_{suffix}")
    }

    fn parse_session_id(session_id: &str) -> Option<(String, String, String, String)> {
        let body = session_id.strip_prefix("session_")?;
        if body.len() < 24 {
            return None;
        }
        let rev: Vec<&str> = body.rsplitn(3, '_').collect();
        if rev.len() < 3 {
            return None;
        }
        let suffix = rev[0];
        let stamp = rev[1];
        if suffix.len() != 8 || stamp.len() != 14 || !stamp.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let prefix = rev[2];
        prefix.split_once('_').map(|(uid, dk)| {
            let dialogue_key = dk.replace('_', ":");
            (
                uid.to_string(),
                dialogue_key,
                stamp.to_string(),
                suffix.to_string(),
            )
        })
    }

    // ── 会话生命周期方法 ──────────────────────────────────────

    /// 构造 dialogue_key — 对应 Python 版 _dialogue_key / build_dialogue_key
    pub fn build_dialogue_key(
        &self,
        user_id: &str,
        dialogue_key: Option<&str>,
        message_type: &str,
        group_id: Option<&str>,
        platform: &str,
    ) -> String {
        let resolved_platform = if platform.trim().is_empty() {
            "qq"
        } else {
            platform.trim()
        };

        if let Some(dk) = dialogue_key {
            if !dk.trim().is_empty() {
                return dk.trim().to_string();
            }
        }

        if message_type == "group" {
            if let Some(gid) = group_id {
                if !gid.trim().is_empty() {
                    return format!("{}:group:{}", resolved_platform, gid);
                }
            }
        }

        format!("{}:private:{}", resolved_platform, user_id)
    }

    /// 检查会话是否已过期
    fn session_expired(&self, session: &SessionRecord) -> bool {
        if session.updated_at.is_empty() {
            return false;
        }
        let updated = match chrono::DateTime::parse_from_rfc3339(&session.updated_at) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => return false,
        };
        let elapsed = (chrono::Utc::now() - updated).num_seconds();
        elapsed > self.session_timeout_seconds as i64
    }

    /// 确保活跃会话存在 — 对应 Python 版 _ensure_session
    /// 返回 (session_id, closed_session_id)
    fn ensure_session(
        &self,
        user_id: &str,
        dialogue_key: &str,
        message_type: &str,
        group_id: &str,
    ) -> (String, String) {
        let mut closed_session_id = String::new();

        // 检查是否已有活跃会话
        {
            let by_dialogue = self
                .active_session_by_dialogue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(active_sid) = by_dialogue.get(dialogue_key) {
                let sessions = self
                    .sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if let Some(session) = sessions.get(active_sid) {
                    if !self.session_expired(session) {
                        return (active_sid.clone(), closed_session_id);
                    }
                }
            }
        }

        // 活跃会话不存在或已过期
        {
            let mut by_dialogue = self
                .active_session_by_dialogue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut sessions = self
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());

            // 如果有过期会话，关闭它
            if let Some(old_sid) = by_dialogue.remove(dialogue_key) {
                if let Some(old_session) = sessions.get_mut(&old_sid) {
                    old_session.closed_at = chrono::Utc::now().to_rfc3339();
                    closed_session_id = old_sid;
                }
            }

            // 创建新会话
            let now = chrono::Utc::now().to_rfc3339();
            let new_sid = self.generate_session_id(user_id, dialogue_key);
            let new_session = SessionRecord {
                session_id: new_sid.clone(),
                dialogue_key: dialogue_key.to_string(),
                user_id: user_id.to_string(),
                message_type: message_type.to_string(),
                group_id: group_id.to_string(),
                started_at: now.clone(),
                updated_at: now,
                closed_at: String::new(),
                turns: Vec::new(),
                metadata: HashMap::new(),
                dirty_turns: 0,
            };

            by_dialogue.insert(dialogue_key.to_string(), new_sid.clone());
            sessions.insert(new_sid.clone(), new_session);

            (new_sid, closed_session_id)
        }
    }

    /// 获取指定会话的拥有者 user_id
    pub fn get_session_owner(&self, session_id: &str) -> String {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions
            .get(session_id)
            .map(|s| s.user_id.clone())
            .unwrap_or_default()
    }

    /// 获取指定会话的快照（深拷贝）
    pub fn get_session_snapshot(&self, session_id: &str) -> Option<SessionRecord> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions.get(session_id).cloned()
    }

    /// 获取指定 dialogue_key 的活跃 session_id
    pub fn get_active_session_id(
        &self,
        user_id: &str,
        dialogue_key: Option<&str>,
        message_type: &str,
        group_id: Option<&str>,
        platform: &str,
    ) -> String {
        let resolved_dk = self.build_dialogue_key(user_id, dialogue_key, message_type, group_id, platform);

        let by_dialogue = self
            .active_session_by_dialogue
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let sid = match by_dialogue.get(&resolved_dk) {
            Some(s) => s.clone(),
            None => return String::new(),
        };

        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        match sessions.get(&sid) {
            Some(session) => {
                if self.session_expired(session) {
                    String::new()
                } else {
                    sid
                }
            }
            None => String::new(),
        }
    }

    /// 获取用户的会话列表（从 SQLite 加载）
    pub async fn get_conversations(
        &self,
        user_id: &str,
        limit: usize,
    ) -> XueliResult<Vec<SessionRecord>> {
        let user_id = user_id.to_string();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let mut stmt = conn
                .prepare(
                    "SELECT session_id, dialogue_key, user_id, message_type, group_id,
                            created_at, closed_at, turn_count, metadata
                     FROM conversation_sessions
                     WHERE user_id = ?1
                     ORDER BY closed_at DESC, created_at DESC
                     LIMIT ?2",
                )
                .map_err(|e| format!("查询失败: {e}"))?;

            let rows: Vec<SessionRecord> = stmt
                .query_map(params![user_id, limit as i64], |row| {
                    let metadata_str: String = row.get(8).unwrap_or_else(|_| "{}".to_string());
                    let metadata_map: HashMap<String, String> =
                        serde_json::from_str(&metadata_str).unwrap_or_default();
                    Ok(SessionRecord {
                        session_id: row.get(0)?,
                        dialogue_key: row.get(1)?,
                        user_id: row.get(2)?,
                        message_type: row.get(3)?,
                        group_id: row.get(4)?,
                        started_at: row.get(5)?,
                        updated_at: String::new(),
                        closed_at: row.get(6)?,
                        turns: Vec::new(),
                        metadata: metadata_map,
                        dirty_turns: 0,
                    })
                })
                .map_err(|e| format!("查询失败: {e}"))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(rows)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    /// 获取群组的会话列表（从 SQLite 加载）
    pub async fn get_conversations_by_group_id(
        &self,
        group_id: &str,
        limit: usize,
    ) -> XueliResult<Vec<SessionRecord>> {
        let group_id = group_id.to_string();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let mut stmt = conn
                .prepare(
                    "SELECT session_id, dialogue_key, user_id, message_type, group_id,
                            created_at, closed_at, turn_count, metadata
                     FROM conversation_sessions
                     WHERE group_id = ?1 AND message_type = 'group'
                     ORDER BY closed_at DESC, created_at DESC
                     LIMIT ?2",
                )
                .map_err(|e| format!("查询失败: {e}"))?;

            let rows: Vec<SessionRecord> = stmt
                .query_map(params![group_id, limit as i64], |row| {
                    let metadata_str: String = row.get(8).unwrap_or_else(|_| "{}".to_string());
                    let metadata_map: HashMap<String, String> =
                        serde_json::from_str(&metadata_str).unwrap_or_default();
                    Ok(SessionRecord {
                        session_id: row.get(0)?,
                        dialogue_key: row.get(1)?,
                        user_id: row.get(2)?,
                        message_type: row.get(3)?,
                        group_id: row.get(4)?,
                        started_at: row.get(5)?,
                        updated_at: String::new(),
                        closed_at: row.get(6)?,
                        turns: Vec::new(),
                        metadata: metadata_map,
                        dirty_turns: 0,
                    })
                })
                .map_err(|e| format!("查询失败: {e}"))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(rows)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    /// 获取群聊最近消息 — 对应 Python 版 get_recent_group_messages
    pub async fn get_recent_group_messages(
        &self,
        group_id: &str,
        limit: usize,
    ) -> XueliResult<Vec<MessageRecord>> {
        let group_id = group_id.to_string();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let mut stmt = conn
                .prepare(
                    "SELECT user_id, display_name, message_text, raw_text, image_descriptions,
                            message_kind, segments, timestamp, message_id, speaker_role
                     FROM group_messages
                     WHERE group_id = ?1
                     ORDER BY timestamp DESC
                     LIMIT ?2",
                )
                .map_err(|e| format!("查询失败: {e}"))?;

            let mut records: Vec<MessageRecord> = stmt
                .query_map(params![group_id, limit as i64], |row| {
                    let img_str: String = row.get(4)?;
                    let seg_str: String = row.get(6)?;
                    Ok(MessageRecord {
                        user_id: row.get(0)?,
                        display_name: row.get(1)?,
                        message_text: row.get(2)?,
                        raw_text: row.get(3)?,
                        image_descriptions: serde_json::from_str(&img_str).unwrap_or_default(),
                        message_kind: row.get(5)?,
                        segments: serde_json::from_str(&seg_str).unwrap_or_default(),
                        timestamp: row.get(7)?,
                        message_id: row.get(8)?,
                        speaker_role: row.get(9)?,
                    })
                })
                .map_err(|e| format!("查询失败: {e}"))?
                .filter_map(|r| r.ok())
                .collect();

            records.reverse();
            Ok(records)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    /// 获取私聊最近消息 — 对应 Python 版 get_recent_private_messages
    pub async fn get_recent_private_messages(
        &self,
        user_id: &str,
        limit: usize,
    ) -> XueliResult<Vec<MessageRecord>> {
        let user_id = user_id.to_string();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            let mut stmt = conn
                .prepare(
                    "SELECT user_id, display_name, message_text, raw_text, image_descriptions,
                            message_kind, segments, timestamp, message_id, speaker_role
                     FROM private_messages
                     WHERE user_id = ?1
                     ORDER BY timestamp DESC
                     LIMIT ?2",
                )
                .map_err(|e| format!("查询失败: {e}"))?;

            let mut records: Vec<MessageRecord> = stmt
                .query_map(params![user_id, limit as i64], |row| {
                    let img_str: String = row.get(4)?;
                    let seg_str: String = row.get(6)?;
                    Ok(MessageRecord {
                        user_id: row.get(0)?,
                        display_name: row.get(1)?,
                        message_text: row.get(2)?,
                        raw_text: row.get(3)?,
                        image_descriptions: serde_json::from_str(&img_str).unwrap_or_default(),
                        message_kind: row.get(5)?,
                        segments: serde_json::from_str(&seg_str).unwrap_or_default(),
                        timestamp: row.get(7)?,
                        message_id: row.get(8)?,
                        speaker_role: row.get(9)?,
                    })
                })
                .map_err(|e| format!("查询失败: {e}"))?
                .filter_map(|r| r.ok())
                .collect();

            records.reverse();
            Ok(records)
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    /// 清除指定群的所有消息 — 对应 Python 版 clear_group_messages
    pub async fn clear_group_messages(&self, group_id: &str) -> XueliResult<()> {
        let group_id = group_id.to_string();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            conn.execute("DELETE FROM group_messages WHERE group_id = ?1", params![group_id])
                .map_err(|e| format!("删除群聊消息失败: {e}"))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    /// 清除指定用户的所有私聊消息 — 对应 Python 版 clear_private_messages
    pub async fn clear_private_messages(&self, user_id: &str) -> XueliResult<()> {
        let user_id = user_id.to_string();
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {e}"))?;
            conn.execute("DELETE FROM private_messages WHERE user_id = ?1", params![user_id])
                .map_err(|e| format!("删除私聊消息失败: {e}"))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("spawn_blocking 失败: {e}"))?
    }

    /// 获取内存中所有活跃会话 ID
    pub fn active_session_ids_from_cache(&self) -> Vec<String> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        sessions.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_store() -> (SqliteConversationStore, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("临时目录创建失败");
        let store = SqliteConversationStore::open(dir.path()).expect("打开数据库失败");
        (store, dir)
    }

    fn make_record(
        session_id: &str,
        user_id: &str,
        sender: &str,
        text: &str,
        is_bot: bool,
        evt_time: f64,
    ) -> ConversationRecord {
        ConversationRecord {
            id: 0,
            session_id: session_id.to_string(),
            user_id: user_id.to_string(),
            sender_name: sender.to_string(),
            text: text.to_string(),
            is_bot,
            scope_type: "private".to_string(),
            scope_id: String::new(),
            event_time: evt_time,
            message_id: format!("msg_{}", evt_time as i64),
            platform: "qq".to_string(),
        }
    }

    #[test]
    fn test_insert_and_retrieve() {
        let (store, _dir) = setup_test_store();

        let sid = "qq:private:user1";
        store
            .insert_message(&make_record(sid, "user1", "Alice", "你好", false, 100.0))
            .expect("插入失败");
        store
            .insert_message(&make_record(sid, "user1", "Bot", "你好呀！", true, 101.0))
            .expect("插入失败");

        let records = store.get_recent_by_session(sid, 10).expect("查询失败");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].text, "你好");
        assert!(!records[0].is_bot);
        assert_eq!(records[1].text, "你好呀！");
        assert!(records[1].is_bot);
    }

    #[test]
    fn test_get_recent_by_scope() {
        let (store, _dir) = setup_test_store();

        let sid_group = "qq:group:g123";
        for i in 0..5 {
            let mut rec = make_record(
                sid_group,
                &format!("u{}", i),
                &format!("User{}", i),
                &format!("群消息 {}", i),
                false,
                100.0 + i as f64,
            );
            rec.scope_type = "group".to_string();
            rec.scope_id = "g123".to_string();
            store.insert_message(&rec).expect("插入失败");
        }

        let sid_private = "qq:private:user1";
        for i in 0..3 {
            let rec = make_record(
                sid_private,
                "user1",
                "User1",
                &format!("私聊消息 {}", i),
                false,
                200.0 + i as f64,
            );
            store.insert_message(&rec).expect("插入失败");
        }

        let group_records = store
            .get_recent_by_scope("group", "g123", 10)
            .expect("查询失败");
        assert_eq!(group_records.len(), 5);

        let private_records = store
            .get_recent_by_scope("private", "", 10)
            .expect("查询失败");
        assert_eq!(private_records.len(), 3);
    }

    #[test]
    fn test_limit() {
        let (store, _dir) = setup_test_store();
        let sid = "qq:private:user1";

        for i in 0..10 {
            store
                .insert_message(&make_record(
                    sid,
                    "user1",
                    "User",
                    &format!("msg{}", i),
                    false,
                    100.0 + i as f64,
                ))
                .expect("插入失败");
        }

        let records = store.get_recent_by_session(sid, 3).expect("查询失败");
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].text, "msg7");
        assert_eq!(records[2].text, "msg9");
    }

    #[test]
    fn test_batch_insert() {
        let (store, _dir) = setup_test_store();
        let sid = "qq:private:user1";

        let records: Vec<_> = (0..5)
            .map(|i| {
                make_record(
                    sid,
                    "user1",
                    "User",
                    &format!("m{}", i),
                    false,
                    100.0 + i as f64,
                )
            })
            .collect();

        let count = store.insert_messages(&records).expect("批量插入失败");
        assert_eq!(count, 5);

        let fetched = store.get_recent_by_session(sid, 10).expect("查询失败");
        assert_eq!(fetched.len(), 5);
    }

    // ── 会话生命周期测试 ──

    #[tokio::test]
    async fn test_add_turn_and_load_session() {
        let (store, _dir) = setup_test_store();
        let sid = store.generate_session_id("user1", "qq:private:user1");

        let user_msg = MessageRecord::user("user1", "Alice", "你好", 100, "msg001");
        let assistant_msg = MessageRecord::assistant("你好呀！", 101);

        store
            .add_turn(&sid, &user_msg, &assistant_msg)
            .await
            .expect("add_turn 失败");

        let loaded = store.load_session(&sid).await.expect("load_session 失败");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].message_text, "你好");
        assert_eq!(loaded[0].speaker_role, "user");
        assert_eq!(loaded[1].message_text, "你好呀！");
        assert_eq!(loaded[1].speaker_role, "assistant");
    }

    #[tokio::test]
    async fn test_multiple_turns() {
        let (store, _dir) = setup_test_store();
        let sid = store.generate_session_id("user1", "qq:private:user1");

        for i in 0..3 {
            let user_msg = MessageRecord::user(
                "user1",
                "Alice",
                &format!("用户消息{}", i),
                100 + i * 10,
                &format!("msg_{}", i),
            );
            let assistant_msg =
                MessageRecord::assistant(&format!("助手回复{}", i), 100 + i * 10 + 1);
            store
                .add_turn(&sid, &user_msg, &assistant_msg)
                .await
                .expect("add_turn 失败");
        }

        let loaded = store.load_session(&sid).await.expect("load_session 失败");
        assert_eq!(loaded.len(), 6);
    }

    #[tokio::test]
    async fn test_save_and_load_conversation() {
        let (store, _dir) = setup_test_store();
        let sid = store.generate_session_id("user1", "qq:private:user1");

        let messages = vec![
            MessageRecord::user("user1", "Alice", "msg1", 100, "id1"),
            MessageRecord::assistant("reply1", 101),
            MessageRecord::user("user1", "Alice", "msg2", 102, "id2"),
            MessageRecord::assistant("reply2", 103),
        ];

        store
            .save_conversation(&sid, &messages)
            .await
            .expect("save 失败");

        let loaded = store.load_session(&sid).await.expect("load 失败");
        assert_eq!(loaded.len(), 4);
        assert_eq!(loaded[0].message_text, "msg1");
        assert_eq!(loaded[3].message_text, "reply2");
    }

    #[tokio::test]
    async fn test_active_session_ids_and_close() {
        let (store, _dir) = setup_test_store();
        let sid = store.generate_session_id("user1", "qq:private:user1");

        let user_msg = MessageRecord::user("user1", "Alice", "你好", 100, "msg001");
        let assistant_msg = MessageRecord::assistant("你好呀！", 101);
        store
            .add_turn(&sid, &user_msg, &assistant_msg)
            .await
            .expect("add_turn 失败");

        let active = store.active_session_ids().await.expect("active 查询失败");
        assert!(!active.is_empty());

        let closed = store.close_all_sessions().await.expect("关闭失败");
        assert!(!closed.is_empty());

        let active_after = store.active_session_ids().await.expect("active 查询失败");
        assert!(active_after.is_empty());
    }

    #[tokio::test]
    async fn test_close_session_by_user_scope() {
        let (store, _dir) = setup_test_store();
        let sid = store.generate_session_id("user1", "qq:private:user1");

        let user_msg = MessageRecord::user("user1", "Alice", "你好", 100, "msg001");
        let assistant_msg = MessageRecord::assistant("你好呀！", 101);
        store
            .add_turn(&sid, &user_msg, &assistant_msg)
            .await
            .expect("add_turn 失败");

        let closed = store
            .close_session("user1", "qq:private:user1")
            .await
            .expect("关闭失败");
        assert!(closed.is_some());
    }

    #[tokio::test]
    async fn test_update_session_metadata() {
        let (store, _dir) = setup_test_store();
        let sid = store.generate_session_id("user1", "qq:private:user1");

        let user_msg = MessageRecord::user("user1", "Alice", "你好", 100, "msg001");
        let assistant_msg = MessageRecord::assistant("你好呀！", 101);
        store
            .add_turn(&sid, &user_msg, &assistant_msg)
            .await
            .expect("add_turn 失败");

        let mut meta = HashMap::new();
        meta.insert("latest_message_id".to_string(), "msg001".to_string());
        meta.insert("status".to_string(), "active".to_string());

        store
            .update_session_metadata(&sid, &meta)
            .await
            .expect("更新元数据失败");
    }

    #[tokio::test]
    async fn test_add_group_message() {
        let (store, _dir) = setup_test_store();

        let msg = MessageRecord::user("user1", "Alice", "群聊消息", 1000, "msg_g1");
        store
            .add_group_message("g123", &msg)
            .await
            .expect("添加群聊消息失败");

        let msgs = store
            .get_messages_after_id("g123", "0")
            .await
            .expect("查询失败");
        assert!(!msgs.is_empty());
        assert_eq!(msgs[0].message_text, "群聊消息");
    }

    #[tokio::test]
    async fn test_add_private_message() {
        let (store, _dir) = setup_test_store();

        let msg = MessageRecord::user("user1", "Alice", "私聊消息", 1000, "msg_p1");
        store
            .add_private_message("user1", &msg)
            .await
            .expect("添加私聊消息失败");
    }

    #[tokio::test]
    async fn test_get_messages_after_id_pagination() {
        let (store, _dir) = setup_test_store();

        for i in 0..5 {
            let msg = MessageRecord::user(
                "user1",
                "Alice",
                &format!("消息{}", i),
                1000 + i,
                &format!("msg_{}", i),
            );
            store
                .add_group_message("g123", &msg)
                .await
                .expect("添加失败");
        }

        let after_msg3 = store
            .get_messages_after_id("g123", "msg_2")
            .await
            .expect("查询失败");
        assert!(!after_msg3.is_empty());
        assert_eq!(after_msg3[0].message_text, "消息3");
        assert_eq!(after_msg3[1].message_text, "消息4");
    }
}
