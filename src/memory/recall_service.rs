use crate::core::types::MemoryItem;

/// 会话记忆回忆服务
pub struct ConversationRecallService;

impl ConversationRecallService {
    pub fn new() -> Self {
        Self
    }

    /// 为当前对话召回相关记忆
    pub async fn recall(
        &self,
        _user_id: &str,
        _session_id: &str,
        _query: &str,
    ) -> Result<Vec<MemoryItem>, String> {
        // TODO: 实现记忆召回（BM25 + 向量混合检索）
        Ok(Vec::new())
    }
}

impl Default for ConversationRecallService {
    fn default() -> Self {
        Self::new()
    }
}