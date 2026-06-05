use crate::core::types::ImmutableMessage;
use crate::prelude::XueliResult;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ── 内存日志 ──

/// Append-only message log with time-based snapshot support.
///
/// 对应 Python 版 `src.core.immutable_message_log.ImmutableMessageLog`
///
/// 特性：
/// - append-only，不修改不删除历史
/// - 按 event_time 索引
/// - 支持时间点快照
#[derive(Debug, Clone)]
pub struct ImmutableMessageLog {
    pub group_id: String,
    messages: Vec<ImmutableMessage>,
}

impl ImmutableMessageLog {
    pub fn new(group_id: impl Into<String>) -> Self {
        Self {
            group_id: group_id.into(),
            messages: Vec::new(),
        }
    }

    /// 追加消息（单生产者，无需外部锁）
    pub fn append(&mut self, message: ImmutableMessage) {
        self.messages.push(message);
        // 按 event_time 排序保持有序
        self.messages.sort_by(|a, b| {
            a.event_time
                .partial_cmp(&b.event_time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// 获取 before_time 之前（不含）的消息快照
    pub fn get_snapshot(&self, before_time: f64) -> (Vec<ImmutableMessage>, f64) {
        let idx = self
            .messages
            .partition_point(|m| m.event_time < before_time);
        (self.messages[..idx].to_vec(), before_time)
    }

    /// 获取 before_time 及之前（含）的消息快照
    pub fn get_snapshot_before_or_at(&self, before_time: f64) -> (Vec<ImmutableMessage>, f64) {
        let idx = self
            .messages
            .partition_point(|m| m.event_time <= before_time);
        (self.messages[..idx].to_vec(), before_time)
    }

    /// 所有消息的只读副本
    pub fn all_messages(&self) -> Vec<ImmutableMessage> {
        self.messages.clone()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

// ── 持久化日志（SQLite 存储） ──

/// 支持 SQLite 持久化的不可变消息日志
///
/// 对应 Python 版 `src.core.immutable_message_log.PersistentImmutableMessageLog`
/// Python 原版使用 aiosqlite，本项目改为同步 rusqlite + spawn_blocking。
#[derive(Debug)]
pub struct PersistentImmutableMessageLog {
    inner: ImmutableMessageLog,
    db_path: PathBuf,
}

impl PersistentImmutableMessageLog {
    pub fn new(group_id: impl Into<String>, db_path: impl AsRef<Path>) -> Self {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        Self {
            inner: ImmutableMessageLog::new(group_id),
            db_path,
        }
    }

    pub fn group_id(&self) -> &str {
        &self.inner.group_id
    }

    pub fn all_messages(&self) -> Vec<ImmutableMessage> {
        self.inner.all_messages()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 初始化数据库表
    pub fn init_db(&self) -> XueliResult<()> {
        let conn = Connection::open(&self.db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
        conn.execute_batch("PRAGMA journal_mode=WAL")
            .map_err(|e| format!("PRAGMA 失败: {}", e))?;
        conn.execute_batch("PRAGMA busy_timeout=3000")
            .map_err(|e| format!("PRAGMA 失败: {}", e))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                group_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                content TEXT NOT NULL,
                event_time REAL NOT NULL,
                received_time REAL NOT NULL,
                raw_data TEXT,
                display_name TEXT DEFAULT '',
                event_text TEXT DEFAULT '',
                UNIQUE(group_id, message_id)
            )",
            [],
        )
        .map_err(|e| format!("建表失败: {}", e))?;

        // 兼容旧表结构
        for col in &["display_name", "event_text"] {
            let _ = conn.execute(
                &format!("ALTER TABLE messages ADD COLUMN {} TEXT DEFAULT ''", col),
                [],
            );
        }

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_messages_group_time ON messages(group_id, event_time)",
            [],
        )
        .map_err(|e| format!("建索引失败: {}", e))?;

        Ok(())
    }

    /// 从数据库加载历史消息
    pub fn load_from_db(&mut self) -> XueliResult<()> {
        let conn = Connection::open(&self.db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;

        let mut stmt = conn
            .prepare(
                "SELECT message_id, user_id, content, event_time, received_time,
                        raw_data, display_name, event_text
                 FROM messages
                 WHERE group_id = ?1
                 ORDER BY event_time ASC",
            )
            .map_err(|e| format!("查询准备失败: {}", e))?;

        let rows = stmt
            .query_map(params![self.inner.group_id], |row| {
                let raw_data_str: String = row.get(4).unwrap_or_default();
                let raw_data: serde_json::Value =
                    serde_json::from_str(&raw_data_str).unwrap_or_default();
                let display_name: String = row.get(5).unwrap_or_default();
                let event_text: String = row.get(6).unwrap_or_default();

                Ok(ImmutableMessage {
                    message_id: row.get(0)?,
                    user_id: row.get(1)?,
                    content: row.get(2)?,
                    event_time: row.get(3)?,
                    received_time: row.get(4)?,
                    raw_data,
                    display_name,
                    event_text,
                })
            })
            .map_err(|e| format!("查询失败: {}", e))?;

        for row in rows {
            match row {
                Ok(msg) => self.inner.append(msg),
                Err(e) => tracing::warn!("[不可变日志] 加载行失败: {}", e),
            }
        }

        tracing::debug!("[不可变日志] 从数据库加载 {} 条消息", self.inner.len());
        Ok(())
    }

    /// 追加并持久化到数据库
    pub fn persist_append(&mut self, message: ImmutableMessage) -> XueliResult<()> {
        self.inner.append(message.clone());

        let conn = Connection::open(&self.db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
        let raw_data_str =
            serde_json::to_string(&message.raw_data).unwrap_or_else(|_| "{}".to_string());

        conn.execute(
            "INSERT OR REPLACE INTO messages
             (group_id, message_id, user_id, content, event_time, received_time, raw_data, display_name, event_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                self.inner.group_id,
                message.message_id,
                message.user_id,
                message.content,
                message.event_time,
                message.received_time,
                raw_data_str,
                message.display_name,
                message.event_text,
            ],
        )
        .map_err(|e| format!("写入消息失败: {}", e))?;

        Ok(())
    }

    /// 异步加载（内部使用 spawn_blocking）
    pub async fn load_from_db_async(&mut self) -> XueliResult<()> {
        let db_path = self.db_path.clone();
        let group_id = self.inner.group_id.clone();

        let messages = tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;

            let mut stmt = conn
                .prepare(
                    "SELECT message_id, user_id, content, event_time, received_time,
                            raw_data, display_name, event_text
                     FROM messages
                     WHERE group_id = ?1
                     ORDER BY event_time ASC",
                )
                .map_err(|e| format!("查询准备失败: {}", e))?;

            let rows = stmt
                .query_map(params![group_id], |row| {
                    let raw_data_str: String = row.get(5).unwrap_or_default();
                    let raw_data: serde_json::Value =
                        serde_json::from_str(&raw_data_str).unwrap_or_default();
                    let display_name: String = row.get(6).unwrap_or_default();
                    let event_text: String = row.get(7).unwrap_or_default();

                    Ok(ImmutableMessage {
                        message_id: row.get(0)?,
                        user_id: row.get(1)?,
                        content: row.get(2)?,
                        event_time: row.get(3)?,
                        received_time: row.get(4)?,
                        raw_data,
                        display_name,
                        event_text,
                    })
                })
                .map_err(|e| format!("查询失败: {}", e))?;

            let mut result = Vec::new();
            for row in rows {
                match row {
                    Ok(msg) => result.push(msg),
                    Err(e) => tracing::warn!("[不可变日志] 加载行失败: {}", e),
                }
            }
            Ok::<_, String>(result)
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))??;

        for msg in messages {
            self.inner.append(msg);
        }

        tracing::debug!("[不可变日志] 异步加载 {} 条消息", self.inner.len());
        Ok(())
    }

    /// 异步追加并持久化
    pub async fn persist_append_async(&mut self, message: ImmutableMessage) -> XueliResult<()> {
        self.inner.append(message.clone());

        let db_path = self.db_path.clone();
        let group_id = self.inner.group_id.clone();
        let raw_data_str =
            serde_json::to_string(&message.raw_data).unwrap_or_else(|_| "{}".to_string());

        let result: XueliResult<()> = tokio::task::spawn_blocking(move || {
            let conn =
                Connection::open(&db_path).map_err(|e| format!("打开 DB 失败: {}", e))?;
            conn.execute(
                "INSERT OR REPLACE INTO messages
                 (group_id, message_id, user_id, content, event_time, received_time, raw_data, display_name, event_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    group_id,
                    message.message_id,
                    message.user_id,
                    message.content,
                    message.event_time,
                    message.received_time,
                    raw_data_str,
                    message.display_name,
                    message.event_text,
                ],
            )
            .map_err(|e| format!("写入消息失败: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| format!("阻塞任务失败: {}", e))?;

        result?;
        Ok(())
    }

    pub fn get_snapshot(&self, before_time: f64) -> (Vec<ImmutableMessage>, f64) {
        self.inner.get_snapshot(before_time)
    }

    pub fn get_snapshot_before_or_at(&self, before_time: f64) -> (Vec<ImmutableMessage>, f64) {
        self.inner.get_snapshot_before_or_at(before_time)
    }

    /// 追加消息（仅内存）
    pub fn append(&mut self, message: ImmutableMessage) {
        self.inner.append(message);
    }
}

impl std::fmt::Display for ImmutableMessageLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ImmutableMessageLog(key={}, messages={})",
            self.group_id,
            self.len()
        )
    }
}

impl std::fmt::Display for PersistentImmutableMessageLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PersistentImmutableMessageLog(key={}, messages={})",
            self.group_id(),
            self.len()
        )
    }
}

// ── 辅助函数 ──

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_msg(event_time: f64) -> ImmutableMessage {
        ImmutableMessage {
            message_id: format!("m{}", event_time as u64),
            user_id: "u1".to_string(),
            content: format!("msg {}", event_time as u64),
            event_time,
            received_time: event_time,
            raw_data: serde_json::json!({}),
            display_name: String::new(),
            event_text: String::new(),
        }
    }

    // ── ImmutableMessageLog 测试 ──

    #[test]
    fn test_append_and_snapshot() {
        let mut log = ImmutableMessageLog::new("g1");
        for i in 0..5 {
            log.append(make_msg(i as f64));
        }
        let (msgs, _) = log.get_snapshot(2.5);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].event_time, 0.0);
        assert_eq!(msgs[2].event_time, 2.0);
    }

    #[test]
    fn test_append_out_of_order() {
        let mut log = ImmutableMessageLog::new("g1");
        log.append(make_msg(200.0));
        log.append(make_msg(100.0));
        let msgs = log.all_messages();
        assert_eq!(msgs[0].event_time, 100.0);
        assert_eq!(msgs[1].event_time, 200.0);
    }

    #[test]
    fn test_empty_log_snapshot() {
        let log = ImmutableMessageLog::new("g1");
        let (msgs, t) = log.get_snapshot(100.0);
        assert!(msgs.is_empty());
        assert_eq!(t, 100.0);
    }

    #[test]
    fn test_get_snapshot_before_or_at() {
        let mut log = ImmutableMessageLog::new("g1");
        for i in 0..3 {
            log.append(make_msg(i as f64));
        }
        let (msgs, _) = log.get_snapshot_before_or_at(1.0);
        assert_eq!(msgs.len(), 2); // 0.0 and 1.0
    }

    #[test]
    fn test_all_messages() {
        let mut log = ImmutableMessageLog::new("g1");
        log.append(make_msg(0.0));
        log.append(make_msg(1.0));
        assert_eq!(log.all_messages().len(), 2);
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut log = ImmutableMessageLog::new("g1");
        assert!(log.is_empty());
        log.append(make_msg(0.0));
        assert_eq!(log.len(), 1);
        assert!(!log.is_empty());
    }

    #[test]
    fn test_display() {
        let mut log = ImmutableMessageLog::new("g1");
        log.append(make_msg(0.0));
        let s = format!("{}", log);
        assert!(s.contains("g1"));
        assert!(s.contains("1"));
    }

    // ── PersistentImmutableMessageLog 测试 ──

    fn make_persistent() -> (PersistentImmutableMessageLog, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_msgs.db");
        let log = PersistentImmutableMessageLog::new("g1", db_path);
        log.init_db().unwrap();
        (log, dir)
    }

    #[test]
    fn test_persist_append_and_load() {
        let (mut log, _dir) = make_persistent();
        log.persist_append(make_msg(100.0)).unwrap();
        log.persist_append(make_msg(200.0)).unwrap();

        assert_eq!(log.len(), 2);

        // 重新加载
        let mut log2 = PersistentImmutableMessageLog::new("g1", _dir.path().join("test_msgs.db"));
        log2.init_db().unwrap();
        log2.load_from_db().unwrap();
        assert_eq!(log2.len(), 2);
        assert_eq!(log2.all_messages()[0].event_time, 100.0);
    }

    #[test]
    fn test_persist_duplicate_message_id() {
        let (mut log, _dir) = make_persistent();
        let msg = make_msg(100.0);
        log.persist_append(msg.clone()).unwrap();
        log.persist_append(msg).unwrap(); // INSERT OR REPLACE
        assert_eq!(log.len(), 2); // 内存中有两条（因为 append 在 persist 之前）
    }

    #[test]
    fn test_persistent_snapshot() {
        let (mut log, _dir) = make_persistent();
        for i in 0..5 {
            log.persist_append(make_msg(i as f64)).unwrap();
        }
        let (msgs, _) = log.get_snapshot(2.5);
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn test_persistent_display() {
        let (mut log, _dir) = make_persistent();
        log.persist_append(make_msg(0.0)).unwrap();
        let s = format!("{}", log);
        assert!(s.contains("g1"));
    }

    #[tokio::test]
    async fn test_persist_append_async() {
        let (mut log, _dir) = make_persistent();
        log.persist_append_async(make_msg(100.0)).await.unwrap();
        assert_eq!(log.len(), 1);
    }
}
