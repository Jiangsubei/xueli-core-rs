/// 聊天摘要服务 — 对长对话生成摘要
pub struct ChatSummaryService;

impl ChatSummaryService {
    pub fn new() -> Self {
        Self
    }

    pub async fn summarize(
        &self,
        _messages: &[String],
    ) -> Result<String, String> {
        // TODO: 集成 LLM 调用生成摘要
        Ok(String::new())
    }
}

impl Default for ChatSummaryService {
    fn default() -> Self {
        Self::new()
    }
}