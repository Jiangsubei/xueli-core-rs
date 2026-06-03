use crate::core::types::MemoryPatch;

/// LLM 记忆提取器 — 从对话中提取结构化记忆
pub struct MemoryExtractor;

impl MemoryExtractor {
    pub fn new() -> Self {
        Self
    }

    /// 从一组消息中提取记忆
    pub async fn extract(
        &self,
        _user_id: &str,
        _messages: &[String],
    ) -> Result<MemoryPatch, String> {
        // TODO: 集成 LLM 调用提取记忆
        Ok(MemoryPatch {
            add: Vec::new(),
            update: Vec::new(),
            remove: Vec::new(),
        })
    }
}

impl Default for MemoryExtractor {
    fn default() -> Self {
        Self::new()
    }
}