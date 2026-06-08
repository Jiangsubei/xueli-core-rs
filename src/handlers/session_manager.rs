use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex;

use crate::core::platform_types::{InboundEvent, SessionRef};
use crate::core::scope::ChatScope;
use crate::memory::stores::conversation::SqliteConversationStore;

/// 管理私聊和群聊的每会话内存状态，包含对话历史与异步锁。
pub struct ConversationSessionManager {
    /// 按会话键索引的对话对象
    conversations: Arc<Mutex<HashMap<String, ConversationState>>>,
    /// 按会话键的异步锁（保证同一会话串行处理）
    session_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// 持久化对话存储（用于恢复历史会话）
    conversation_store: Option<Arc<SqliteConversationStore>>,
    /// 会话键 → 用户 ID 映射
    owner_user_ids: Arc<StdMutex<HashMap<String, String>>>,
}

/// 内存中的会话状态
#[derive(Debug, Clone)]
pub struct ConversationState {
    /// 消息列表，每条为 (role, content, timestamp)
    pub messages: Vec<MessageEntry>,
    /// 最后更新时间（Unix 秒）
    pub last_update: f64,
    /// 恢复的历史会话时间
    pub restored_previous_session_time: f64,
    /// 恢复的会话 ID
    pub restored_session_id: String,
    /// 恢复的最后一条消息时间
    pub restored_last_message_time: Option<f64>,
    /// 是否有待处理的恢复会话
    pub restored_session_pending: bool,
}

/// 消息条目
#[derive(Debug, Clone)]
pub struct MessageEntry {
    pub role: String,
    pub content: String,
    pub timestamp: f64,
    pub image_description: String,
    pub message_id: String,
    pub restored: bool,
}

impl Default for ConversationState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            last_update: current_timestamp(),
            restored_previous_session_time: 0.0,
            restored_session_id: String::new(),
            restored_last_message_time: None,
            restored_session_pending: false,
        }
    }
}

fn current_timestamp() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

impl ConversationSessionManager {
    pub fn new(conversation_store: Option<Arc<SqliteConversationStore>>) -> Self {
        Self {
            conversations: Arc::new(Mutex::new(HashMap::new())),
            session_locks: Arc::new(Mutex::new(HashMap::new())),
            conversation_store,
            owner_user_ids: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    /// 从会话引用生成会话键
    pub fn get_key_for_session(&self, session: &SessionRef) -> String {
        let key = match &session.scope {
            ChatScope::Group(gid) => {
                format!(
                    "{}:group:{}",
                    &session.session_id.split(':').next().unwrap_or("unknown"),
                    gid
                )
            }
            ChatScope::Private => {
                let uid = session.user_id.as_deref().unwrap_or("");
                format!(
                    "{}:private:{}",
                    &session.session_id.split(':').next().unwrap_or("unknown"),
                    uid
                )
            }
        };
        let owner_user_id = session.user_id.clone().unwrap_or_default();
        if !key.is_empty() && !owner_user_id.is_empty() {
            if let Ok(mut owner_map) = self.owner_user_ids.lock() {
                owner_map.insert(key.clone(), owner_user_id);
            }
        }
        key
    }

    /// 从入站事件生成会话键
    pub fn get_key_for_event(&self, event: &InboundEvent) -> String {
        self.get_key_for_session(&event.get_session())
    }

    /// 获取或创建指定键的会话状态（若不存在则新建）
    pub async fn get_or_create(&self, key: &str) -> ConversationState {
        let mut conversations = self.conversations.lock().await;
        if let Some(conv) = conversations.get(key) {
            conv.clone()
        } else {
            let state = ConversationState::default();
            conversations.insert(key.to_string(), state.clone());
            state
        }
    }

    /// 更新指定键的会话状态
    pub async fn update(&self, key: &str, state: ConversationState) {
        let mut conversations = self.conversations.lock().await;
        conversations.insert(key.to_string(), state);
    }

    /// 向指定会话添加一条消息
    pub async fn add_message(
        &self,
        key: &str,
        role: &str,
        content: &str,
        timestamp: Option<f64>,
        image_description: &str,
        message_id: &str,
        restored: bool,
    ) {
        let mut conversations = self.conversations.lock().await;
        let conv = conversations.entry(key.to_string()).or_default();
        let event_time = timestamp.unwrap_or_else(current_timestamp);
        conv.messages.push(MessageEntry {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: event_time,
            image_description: image_description.to_string(),
            message_id: message_id.to_string(),
            restored,
        });
        conv.last_update = event_time;
        if restored {
            conv.restored_last_message_time = Some(
                conv.restored_last_message_time
                    .unwrap_or(0.0)
                    .max(event_time),
            );
        } else if conv.restored_session_pending {
            conv.restored_session_pending = false;
        }
    }

    /// 获取指定键会话最近的若干条消息（不做副本拷贝，返回引用）
    pub async fn get_recent_messages(&self, key: &str, max_count: usize) -> Vec<MessageEntry> {
        let conversations = self.conversations.lock().await;
        if let Some(conv) = conversations.get(key) {
            let start = if conv.messages.len() > max_count {
                conv.messages.len() - max_count
            } else {
                0
            };
            conv.messages[start..].to_vec()
        } else {
            vec![]
        }
    }

    /// 获取指定会话键的异步锁（保证同一会话串行处理）
    pub async fn get_session_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.session_locks.lock().await;
        locks
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// 清除指定会话
    pub async fn clear(&self, key: &str) -> bool {
        let mut conversations = self.conversations.lock().await;
        conversations.remove(key).is_some()
    }

    /// 清除事件对应的会话
    pub async fn clear_for_event(&self, event: &InboundEvent) -> bool {
        let key = self.get_key_for_event(event);
        self.clear(&key).await
    }

    /// 清理超过 6 小时无活动且无消息的空闲会话
    pub async fn clean_expired(&self) {
        let now = current_timestamp();
        let mut conversations = self.conversations.lock().await;
        let stale_keys: Vec<String> = conversations
            .iter()
            .filter(|(_k, v)| v.messages.is_empty() && (now - v.last_update) > 6.0 * 3600.0)
            .map(|(k, _)| k.clone())
            .collect();
        for key in stale_keys {
            conversations.remove(&key);
        }
    }

    /// 活跃会话数
    pub async fn count_active(&self) -> usize {
        let conversations = self.conversations.lock().await;
        conversations.len()
    }

    /// 从持久化存储恢复历史会话到内存（用于空会话初始化）。
    ///
    /// 群聊：按 scope 查询群组内所有历史消息。
    /// 私聊：按用户 ID 查询该用户的所有历史消息。
    ///
    /// 恢复后设置 restored_session_pending 标识，供 downstream 判断是否为恢复上下文。
    pub async fn restore(&self, key: &str) {
        let store = match &self.conversation_store {
            Some(s) => s.clone(),
            None => return,
        };

        let parts: Vec<String> = key.split(':').map(|s| s.to_string()).collect();
        let is_group = parts.len() >= 3 && parts.get(1).map_or(false, |s| s == "group");

        let fetch_key = key.to_string();
        let records_result = if is_group {
            let group_id = parts.get(2).cloned().unwrap_or_default();
            if group_id.is_empty() {
                return;
            }
            let store_arc = store.clone();
            tokio::task::spawn_blocking(move || {
                store_arc.get_recent_by_scope("group", &group_id, 600)
            })
            .await
        } else {
            let user_id = self._resolve_owner_user_id(key);
            if user_id.is_empty() {
                return;
            }
            let store_arc = store.clone();
            tokio::task::spawn_blocking(move || store_arc.get_recent_by_user(&user_id, 600)).await
        };

        let records = match records_result {
            Ok(Ok(recs)) => recs,
            _ => {
                tracing::warn!("[会话管理器] 加载历史会话失败, key={}", fetch_key);
                return;
            }
        };

        if records.is_empty() {
            return;
        }

        let mut latest_time = 0.0f64;
        let mut latest_session_id = String::new();
        let mut messages: Vec<MessageEntry> = Vec::new();

        for record in &records {
            let text = record.text.trim();
            if text.is_empty() {
                continue;
            }
            let event_time = record.event_time;
            if event_time > 0.0 && event_time > latest_time {
                latest_time = event_time;
                latest_session_id = record.session_id.clone();
            }
            let role = if record.is_bot { "assistant" } else { "user" };
            messages.push(MessageEntry {
                role: role.to_string(),
                content: text.to_string(),
                timestamp: event_time,
                image_description: String::new(),
                message_id: record.message_id.clone(),
                restored: true,
            });
        }

        messages.sort_by(|a, b| {
            a.timestamp
                .partial_cmp(&b.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if messages.is_empty() {
            return;
        }

        let message_count = messages.len();
        let mut conversations = self.conversations.lock().await;
        let conv = conversations.entry(fetch_key).or_default();
        for msg in &messages {
            conv.messages.push(msg.clone());
        }
        conv.restored_previous_session_time = latest_time;
        conv.restored_session_id = latest_session_id;
        conv.restored_session_pending = true;
        conv.last_update = current_timestamp();

        tracing::debug!("[会话管理器] 已恢复历史会话，共 {} 条消息", message_count);
    }

    /// 获取或恢复会话（带双检锁的并发安全）。
    ///
    /// 1. 快速路径：已存在且非空或无存储 → 直接返回
    /// 2. 获取会话级异步锁
    /// 3. 双检：已被其他任务恢复则直接返回
    /// 4. 创建空会话并在锁外恢复
    pub async fn get_or_restore(&self, key: &str) -> ConversationState {
        {
            let conversations = self.conversations.lock().await;
            if let Some(conv) = conversations.get(key) {
                if !conv.messages.is_empty() || self.conversation_store.is_none() {
                    return conv.clone();
                }
            }
        }

        let session_lock = self.get_session_lock(key).await;
        let _guard = session_lock.lock().await;

        {
            let conversations = self.conversations.lock().await;
            if let Some(conv) = conversations.get(key) {
                if !conv.messages.is_empty() {
                    return conv.clone();
                }
            }
        }

        {
            let mut conversations = self.conversations.lock().await;
            conversations.entry(key.to_string()).or_default();
        }

        self.restore(key).await;

        self.get_or_create(key).await
    }

    /// 从 owner_user_ids 或键中解析用户 ID
    fn _resolve_owner_user_id(&self, key: &str) -> String {
        if let Ok(owner_map) = self.owner_user_ids.lock() {
            if let Some(uid) = owner_map.get(key) {
                if !uid.is_empty() {
                    return uid.clone();
                }
            }
        }
        self._extract_user_id_from_key(key)
    }

    /// 从会话键提取用户 ID（fallback）
    fn _extract_user_id_from_key(&self, key: &str) -> String {
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() >= 3 {
            if parts.get(1) == Some(&"group") {
                return parts.get(2).unwrap_or(&"").to_string();
            }
            return parts.last().unwrap_or(&"").to_string();
        }
        parts.last().unwrap_or(&"").to_string()
    }
}

impl Default for ConversationSessionManager {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_or_create() {
        let mgr = ConversationSessionManager::new(None);
        let conv = mgr.get_or_create("key1").await;
        assert!(conv.messages.is_empty());
        assert!(conv.last_update > 0.0);
    }

    #[tokio::test]
    async fn test_add_and_get_messages() {
        let mgr = ConversationSessionManager::new(None);
        mgr.add_message("key1", "user", "你好", None, "", "", false)
            .await;
        mgr.add_message("key1", "assistant", "你好！", None, "", "", false)
            .await;

        let msgs = mgr.get_recent_messages("key1", 10).await;
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, "你好");
        assert_eq!(msgs[1].content, "你好！");
    }

    #[tokio::test]
    async fn test_clear() {
        let mgr = ConversationSessionManager::new(None);
        mgr.add_message("key1", "user", "hi", None, "", "", false)
            .await;
        assert_eq!(mgr.count_active().await, 1);
        mgr.clear("key1").await;
        assert_eq!(mgr.count_active().await, 0);
    }

    #[tokio::test]
    async fn test_clean_expired() {
        let mgr = ConversationSessionManager::new(None);
        // 创建空会话并手动回退 last_update 模拟过期
        {
            let mut conversations = mgr.conversations.lock().await;
            let mut state = ConversationState::default();
            state.last_update = current_timestamp() - 7.0 * 3600.0; // 7 小时前
            conversations.insert("stale".into(), state);
        }
        mgr.clean_expired().await;
        assert_eq!(mgr.count_active().await, 0);
    }

    #[tokio::test]
    async fn test_session_key_for_event() {
        let mgr = ConversationSessionManager::new(None);
        let event = InboundEvent {
            id: "e1".into(),
            platform: "test".into(),
            event_type: crate::core::platform_types::EventType::Message,
            message: Some(crate::core::types::UserMessage {
                id: "m1".into(),
                sender_id: "u1".into(),
                sender_name: "T".into(),
                text: "hi".into(),
                timestamp: chrono::Utc::now(),
                scope: ChatScope::Group("g1".into()),
                is_mention: false,
            }),
            raw_payload: None,
            received_at: chrono::Utc::now(),
            session: None,
            ..Default::default()
        };
        let key = mgr.get_key_for_event(&event);
        assert!(key.contains("group"));
    }
}
