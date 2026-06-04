use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::Semaphore;

use crate::prelude::XueliResult;

/// 统一的消息记录 — 覆盖群聊和私聊
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRecord {
    /// 自增主键
    pub id: i64,
    /// 会话标识（按 dialogue_key 格式，如 "qq:group:123" 或 "qq:private:uid"）
    pub session_id: String,
    /// 用户 ID
    pub user_id: String,
    /// 发送者昵称
    pub sender_name: String,
    /// 消息文本
    pub text: String,
    /// 是否 bot 回复
    pub is_bot: bool,
    /// 作用域类型：private / group
    pub scope_type: String,
    /// 群组 ID（群聊时使用）
    pub scope_id: String,
    /// 事件时间（Unix 时间戳，秒）
    pub event_time: f64,
    /// 平台消息 ID
    pub message_id: String,
    /// 平台名称
    pub platform: String,
}

impl ConversationRecord {
    /// 从入站消息构造记录（非 bot 发言）
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

    /// 从 bot 回复构造记录
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

/// SQLite 对话存储 — 统一管理群聊/私聊消息
pub struct SqliteConversationStore {
    conn: Mutex<Connection>,
    /// 写并发限制
    _write_sem: Semaphore,
}

const INIT_SCHEMA: &str = "
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
";

impl SqliteConversationStore {
    /// 打开数据库并初始化
    pub fn open(db_dir: &Path) -> XueliResult<Self> {
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
            _write_sem: Semaphore::new(5),
        })
    }

    /// 插入一条消息
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

    /// 批量插入消息
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

    /// 按 session_id 获取最近 N 条消息
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
        // 反转为时间升序（从旧到新）
        records.reverse();
        Ok(records)
    }

    /// 按作用域获取最近 N 条消息（群聊或私聊）
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

    /// 按用户 ID 获取最近 N 条消息
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_store() -> SqliteConversationStore {
        let dir = tempfile::TempDir::new().expect("临时目录创建失败");
        SqliteConversationStore::open(dir.path()).expect("打开数据库失败")
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
        let store = setup_test_store();

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
        let store = setup_test_store();

        // 插入群聊消息
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

        // 插入私聊消息
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

        // 按群聊 scope 查询
        let group_records = store
            .get_recent_by_scope("group", "g123", 10)
            .expect("查询失败");
        assert_eq!(group_records.len(), 5);

        // 按私聊 scope 查询
        let private_records = store
            .get_recent_by_scope("private", "", 10)
            .expect("查询失败");
        assert_eq!(private_records.len(), 3);
    }

    #[test]
    fn test_limit() {
        let store = setup_test_store();
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
        // 应该是最新的 3 条: msg7, msg8, msg9
        assert_eq!(records[0].text, "msg7");
        assert_eq!(records[2].text, "msg9");
    }

    #[test]
    fn test_batch_insert() {
        let store = setup_test_store();
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
}
