use dashmap::DashMap;

use crate::core::platform_types::InboundEvent;

/// 群聊消息收集器 — 收集群聊上下文中的多条消息
pub struct GroupMessageCollector {
    /// group_id → messages
    buffers: DashMap<String, Vec<InboundEvent>>,
    max_buffer_size: usize,
}

impl GroupMessageCollector {
    pub fn new(max_buffer_size: usize) -> Self {
        Self {
            buffers: DashMap::new(),
            max_buffer_size,
        }
    }

    /// 添加消息到缓冲区
    pub fn add(&self, event: InboundEvent) {
        let group_id = event
            .message
            .as_ref()
            .and_then(|m| m.scope.group_id().map(|s| s.to_string()))
            .unwrap_or_default();

        let mut buffer = self.buffers.entry(group_id).or_insert_with(Vec::new);
        buffer.push(event);

        if buffer.len() > self.max_buffer_size {
            buffer.remove(0);
        }
    }

    /// 获取指定群聊的全部缓冲消息
    pub fn get(&self, group_id: &str) -> Vec<InboundEvent> {
        self.buffers
            .get(group_id)
            .map(|b| b.clone())
            .unwrap_or_default()
    }

    /// 清空指定群聊缓冲
    pub fn clear(&self, group_id: &str) {
        self.buffers.remove(group_id);
    }
}

impl Default for GroupMessageCollector {
    fn default() -> Self {
        Self::new(20)
    }
}