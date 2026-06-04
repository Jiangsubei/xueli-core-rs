use dashmap::DashMap;

use crate::core::platform_types::InboundEvent;
use crate::core::scope::ChatScope;

/// 群聊消息收集器 — 收集群聊上下文中的多条消息
///
/// 对应 Python 版 `xueli/src/handlers/group_message_collector.py`
pub struct GroupMessageCollector {
    /// group_key → messages
    buffers: DashMap<String, Vec<InboundEvent>>,
    max_buffer_size: usize,
    /// group_key → latest_message_id（用于 Wait 锚点）
    latest_ids: DashMap<String, String>,
}

impl GroupMessageCollector {
    pub fn new(max_buffer_size: usize) -> Self {
        Self {
            buffers: DashMap::new(),
            max_buffer_size,
            latest_ids: DashMap::new(),
        }
    }

    /// 从事件中提取 group_key
    pub fn group_key_from_event(event: &InboundEvent) -> Option<String> {
        event.message.as_ref().and_then(|m| match &m.scope {
            ChatScope::Group(group_id) => Some(format!("group:{}", group_id)),
            _ => None,
        })
    }

    /// 收集一条群聊消息到缓冲区（借用版本）
    pub fn collect(&self, event: &InboundEvent) -> Option<String> {
        let group_key = Self::group_key_from_event(event)?;

        if let Some(ref msg) = event.message {
            self.latest_ids.insert(group_key.clone(), msg.id.clone());
        }

        let mut buffer = self
            .buffers
            .entry(group_key.clone())
            .or_insert_with(Vec::new);
        buffer.push(event.clone());

        while buffer.len() > self.max_buffer_size {
            buffer.remove(0);
        }

        Some(group_key)
    }

    /// 添加消息到缓冲区（返回 group_key）
    pub fn add(&self, event: InboundEvent) -> Option<String> {
        let group_key = Self::group_key_from_event(&event)?;

        if let Some(ref msg) = event.message {
            self.latest_ids.insert(group_key.clone(), msg.id.clone());
        }

        let mut buffer = self
            .buffers
            .entry(group_key.clone())
            .or_insert_with(Vec::new);
        buffer.push(event);

        if buffer.len() > self.max_buffer_size {
            buffer.remove(0);
        }

        Some(group_key)
    }

    /// 获取指定群聊的全部缓冲消息
    pub fn get(&self, group_key: &str) -> Vec<InboundEvent> {
        self.buffers
            .get(group_key)
            .map(|b| b.clone())
            .unwrap_or_default()
    }

    /// 获取指定群聊的消息数量
    pub fn count(&self, group_key: &str) -> usize {
        self.buffers.get(group_key).map(|b| b.len()).unwrap_or(0)
    }

    /// 获取最新的消息 ID（用于 Wait 锚点）
    pub fn get_latest_message_id(&self, group_key: &str) -> Option<String> {
        self.latest_ids.get(group_key).map(|id| id.clone())
    }

    /// 清空指定群聊缓冲
    pub fn clear(&self, group_key: &str) {
        self.buffers.remove(group_key);
        self.latest_ids.remove(group_key);
    }

    /// 清空所有群聊缓冲
    pub fn clear_all(&self) {
        self.buffers.clear();
        self.latest_ids.clear();
    }

    /// 是否有指定群聊的缓冲消息
    pub fn has_pending(&self, group_key: &str) -> bool {
        self.buffers.contains_key(group_key)
    }
}

impl Default for GroupMessageCollector {
    fn default() -> Self {
        Self::new(20)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::platform_types::EventType;
    use crate::core::types::UserMessage;
    use chrono::Utc;

    fn make_group_event(group_id: &str, msg_id: &str, text: &str) -> InboundEvent {
        InboundEvent {
            id: msg_id.to_string(),
            platform: "test".to_string(),
            event_type: EventType::Message,
            message: Some(UserMessage {
                id: msg_id.to_string(),
                sender_id: "user1".to_string(),
                sender_name: "用户".to_string(),
                text: text.to_string(),
                timestamp: Utc::now(),
                scope: ChatScope::Group(group_id.to_string()),
                is_mention: false,
            }),
            raw_payload: None,
            received_at: Utc::now(),
        }
    }

    #[test]
    fn test_collect_and_get() {
        let collector = GroupMessageCollector::new(5);
        let event = make_group_event("g1", "1", "hello");
        let key = collector.add(event).unwrap();
        assert!(key.contains("g1"));
        assert_eq!(collector.count(&key), 1);
    }

    #[test]
    fn test_max_buffer_size() {
        let collector = GroupMessageCollector::new(3);
        let key = "group:g1";
        for i in 0..5 {
            collector.add(make_group_event("g1", &i.to_string(), &format!("msg{}", i)));
        }
        assert_eq!(collector.count(key), 3);
    }

    #[test]
    fn test_latest_message_id() {
        let collector = GroupMessageCollector::new(10);
        let key = collector
            .add(make_group_event("g1", "100", "first"))
            .unwrap();
        assert_eq!(
            collector.get_latest_message_id(&key).as_deref(),
            Some("100")
        );

        collector.add(make_group_event("g1", "200", "second"));
        assert_eq!(
            collector.get_latest_message_id(&key).as_deref(),
            Some("200")
        );
    }

    #[test]
    fn test_clear() {
        let collector = GroupMessageCollector::new(10);
        let key = collector.add(make_group_event("g1", "1", "hello")).unwrap();
        assert_eq!(collector.count(&key), 1);

        collector.clear(&key);
        assert_eq!(collector.count(&key), 0);
        assert!(collector.get_latest_message_id(&key).is_none());
    }

    #[test]
    fn test_private_event_ignored() {
        let collector = GroupMessageCollector::new(10);
        let event = InboundEvent {
            id: "1".to_string(),
            platform: "test".to_string(),
            event_type: EventType::Message,
            message: Some(UserMessage {
                id: "1".to_string(),
                sender_id: "user1".to_string(),
                sender_name: "用户".to_string(),
                text: "hello".to_string(),
                timestamp: Utc::now(),
                scope: ChatScope::Private,
                is_mention: false,
            }),
            raw_payload: None,
            received_at: Utc::now(),
        };
        assert!(collector.add(event).is_none());
    }
}
