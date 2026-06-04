use crate::prelude::XueliResult;
use crate::core::immutable_message_log::{ImmutableMessageLog, PersistentImmutableMessageLog};
use crate::core::types::{ConversationSnapshot, ImmutableMessage};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// 上下文记录器（单一写入者）
///
/// 对应 Python 版 `src.core.context_recorder.ContextRecorder`
///
/// 职责：
/// - 接收所有群聊消息，按时间顺序追加写入不可变日志
/// - 提供时间点快照读取接口
/// - 单生产者模式，消并发写入竞态
pub struct ContextRecorder {
    /// group_id → 不可变消息日志
    logs: Arc<Mutex<HashMap<String, Box<dyn ImmutableLogTrait + Send>>>>,
    /// group_id → 互斥锁（用于并发创建保护）
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    /// 数据库路径（启用持久化时使用）
    db_path: Option<String>,
}

/// 不可变日志的 trait 抽象
pub trait ImmutableLogTrait: Send + Sync {
    fn group_id(&self) -> &str;
    fn append(&mut self, message: ImmutableMessage);
    fn get_snapshot(&self, before_time: f64) -> (Vec<ImmutableMessage>, f64);
    fn all_messages(&self) -> Vec<ImmutableMessage>;
    fn init_db(&self) -> XueliResult<()>;
    fn load_from_db(&mut self) -> XueliResult<()>;
    fn persist_append(&mut self, message: ImmutableMessage) -> XueliResult<()>;
}

// ── 为 ImmutableMessageLog 实现 trait ──

impl ImmutableLogTrait for ImmutableMessageLog {
    fn group_id(&self) -> &str {
        &self.group_id
    }

    fn append(&mut self, message: ImmutableMessage) {
        self.append(message);
    }

    fn get_snapshot(&self, before_time: f64) -> (Vec<ImmutableMessage>, f64) {
        self.get_snapshot(before_time)
    }

    fn all_messages(&self) -> Vec<ImmutableMessage> {
        self.all_messages()
    }

    fn init_db(&self) -> XueliResult<()> {
        Ok(())
    }

    fn load_from_db(&mut self) -> XueliResult<()> {
        Ok(())
    }

    fn persist_append(&mut self, message: ImmutableMessage) -> XueliResult<()> {
        self.append(message);
        Ok(())
    }
}

// ── 为 PersistentImmutableMessageLog 实现 trait ──

impl ImmutableLogTrait for PersistentImmutableMessageLog {
    fn group_id(&self) -> &str {
        self.group_id()
    }

    fn append(&mut self, message: ImmutableMessage) {
        self.append(message);
    }

    fn get_snapshot(&self, before_time: f64) -> (Vec<ImmutableMessage>, f64) {
        self.get_snapshot(before_time)
    }

    fn all_messages(&self) -> Vec<ImmutableMessage> {
        self.all_messages()
    }

    fn init_db(&self) -> XueliResult<()> {
        self.init_db()
    }

    fn load_from_db(&mut self) -> XueliResult<()> {
        self.load_from_db()
    }

    fn persist_append(&mut self, message: ImmutableMessage) -> XueliResult<()> {
        self.persist_append(message)
    }
}

/// 创建日志后的摘要信息
pub struct LogInfo {
    pub group_id: String,
    pub persistent: bool,
}

impl ContextRecorder {
    pub fn new(db_path: Option<String>) -> Self {
        Self {
            logs: Arc::new(Mutex::new(HashMap::new())),
            locks: Arc::new(Mutex::new(HashMap::new())),
            db_path,
        }
    }

    /// 创建新的不可变日志（内部工厂方法）
    fn create_log(&self, group_id: &str) -> Box<dyn ImmutableLogTrait + Send> {
        if let Some(ref db_path) = self.db_path {
            let path = std::path::Path::new(db_path);
            let mut persistent = PersistentImmutableMessageLog::new(group_id, path);
            if let Err(e) = persistent.init_db() {
                tracing::warn!("[上下文记录器] 初始化持久化日志失败: {}", e);
            }
            if let Err(e) = persistent.load_from_db() {
                tracing::warn!("[上下文记录器] 加载持久化日志失败: {}", e);
            }
            Box::new(persistent)
        } else {
            Box::new(ImmutableMessageLog::new(group_id))
        }
    }

    /// 获取或创建不可变日志
    pub async fn get_or_create_log(&self, group_id: &str) -> LogInfo {
        // 快速路径：已存在
        {
            let logs = self.logs.lock().await;
            if let Some(log) = logs.get(group_id) {
                let persistent = self.db_path.is_some();
                return LogInfo {
                    group_id: log.group_id().to_string(),
                    persistent,
                };
            }
        }

        // 获取或创建锁
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(group_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        let _guard = lock.lock().await;

        // 双重检查
        {
            let logs = self.logs.lock().await;
            if let Some(log) = logs.get(group_id) {
                let persistent = self.db_path.is_some();
                return LogInfo {
                    group_id: log.group_id().to_string(),
                    persistent,
                };
            }
        }

        let log = self.create_log(group_id);
        let group_id_owned = log.group_id().to_string();
        let persistent = self.db_path.is_some();

        let mut logs = self.logs.lock().await;
        logs.insert(group_id.to_string(), log);

        LogInfo {
            group_id: group_id_owned,
            persistent,
        }
    }

    /// 记录消息到不可变日志
    pub async fn record(
        &self,
        group_id: &str,
        message_id: &str,
        user_id: &str,
        content: &str,
        event_time: f64,
        received_time: Option<f64>,
        raw_data: Option<serde_json::Value>,
        display_name: &str,
        event_text: &str,
    ) -> XueliResult<()> {
        let message = ImmutableMessage {
            message_id: message_id.to_string(),
            user_id: user_id.to_string(),
            content: content.to_string(),
            event_time,
            received_time: received_time.unwrap_or_else(now_secs),
            raw_data: raw_data.unwrap_or_default(),
            display_name: display_name.to_string(),
            event_text: event_text.to_string(),
        };

        let mut logs = self.logs.lock().await;
        if let Some(log) = logs.get_mut(group_id) {
            log.persist_append(message)
        } else {
            Err(format!("组 {} 的日志不存在", group_id).into())
        }
    }

    /// 获取时间点快照
    pub async fn get_snapshot(
        &self,
        group_id: &str,
        before_time: f64,
    ) -> (Vec<ImmutableMessage>, f64) {
        let logs = self.logs.lock().await;
        match logs.get(group_id) {
            Some(log) => log.get_snapshot(before_time),
            None => (vec![], before_time),
        }
    }

    /// 获取事件对应时间点的快照
    pub async fn get_snapshot_at(&self, group_id: &str, event_time: f64) -> ConversationSnapshot {
        let (messages, snapshot_time) = self.get_snapshot(group_id, event_time).await;
        ConversationSnapshot {
            conversation_id: group_id.to_string(),
            messages,
            snapshot_time,
            created_at: now_secs(),
        }
    }

    /// 获取完整历史（用于分析/调试）
    pub async fn get_full_history(&self, group_id: &str) -> Vec<ImmutableMessage> {
        let logs = self.logs.lock().await;
        match logs.get(group_id) {
            Some(log) => log.all_messages(),
            None => vec![],
        }
    }

    /// 关闭记录器，清理资源
    pub async fn close(&self) {
        let mut logs = self.logs.lock().await;
        logs.clear();
        let mut locks = self.locks.lock().await;
        locks.clear();
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_or_create_log_creates_new() {
        let recorder = ContextRecorder::new(None);
        let info = recorder.get_or_create_log("g1").await;
        assert_eq!(info.group_id, "g1");
        assert!(!info.persistent);
    }

    #[tokio::test]
    async fn test_get_or_create_log_returns_same_group() {
        let recorder = ContextRecorder::new(None);
        let _ = recorder.get_or_create_log("g1").await;
        let _ = recorder.get_or_create_log("g1").await;
        let logs = recorder.logs.lock().await;
        assert_eq!(logs.len(), 1);
    }

    #[tokio::test]
    async fn test_different_groups_get_different_logs() {
        let recorder = ContextRecorder::new(None);
        let _ = recorder.get_or_create_log("g1").await;
        let _ = recorder.get_or_create_log("g2").await;
        let logs = recorder.logs.lock().await;
        assert_eq!(logs.len(), 2);
    }

    #[tokio::test]
    async fn test_record_and_get_snapshot() {
        let recorder = ContextRecorder::new(None);
        let _ = recorder.get_or_create_log("g1").await;

        recorder
            .record("g1", "m1", "u1", "hello", 100.0, None, None, "", "")
            .await
            .unwrap();

        let (msgs, _) = recorder.get_snapshot("g1", 200.0).await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "hello");
    }

    #[tokio::test]
    async fn test_get_snapshot_unknown_group() {
        let recorder = ContextRecorder::new(None);
        let (msgs, t) = recorder.get_snapshot("unknown", 100.0).await;
        assert!(msgs.is_empty());
        assert_eq!(t, 100.0);
    }

    #[tokio::test]
    async fn test_get_snapshot_at() {
        let recorder = ContextRecorder::new(None);
        let _ = recorder.get_or_create_log("g1").await;

        recorder
            .record("g1", "m1", "u1", "older", 500.0, None, None, "", "")
            .await
            .unwrap();

        let snapshot = recorder.get_snapshot_at("g1", 1000.0).await;
        assert_eq!(snapshot.conversation_id, "g1");
        assert_eq!(snapshot.messages.len(), 1);
        assert!(snapshot.created_at > 0.0);
    }

    #[tokio::test]
    async fn test_get_full_history() {
        let recorder = ContextRecorder::new(None);
        let _ = recorder.get_or_create_log("g1").await;

        recorder
            .record("g1", "m1", "u1", "msg1", 100.0, None, None, "", "")
            .await
            .unwrap();
        recorder
            .record("g1", "m2", "u1", "msg2", 200.0, None, None, "", "")
            .await
            .unwrap();

        let history = recorder.get_full_history("g1").await;
        assert_eq!(history.len(), 2);
    }

    #[tokio::test]
    async fn test_get_full_history_unknown() {
        let recorder = ContextRecorder::new(None);
        let history = recorder.get_full_history("unknown").await;
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_close_clears_logs() {
        let recorder = ContextRecorder::new(None);
        let _ = recorder.get_or_create_log("g1").await;
        let _ = recorder.get_or_create_log("g2").await;

        {
            let logs = recorder.logs.lock().await;
            assert_eq!(logs.len(), 2);
        }

        recorder.close().await;

        let logs = recorder.logs.lock().await;
        assert!(logs.is_empty());
    }

    #[tokio::test]
    async fn test_close_idempotent() {
        let recorder = ContextRecorder::new(None);
        recorder.close().await;
        recorder.close().await;
        // 不 panic
    }
}
