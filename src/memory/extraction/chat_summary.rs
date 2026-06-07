use std::sync::Arc;
use tracing::debug;

use crate::prelude::XueliResult;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage, MessageContent};

/// 聊天摘要服务 — 对长对话生成摘要
pub struct ChatSummaryService<A: AIClient> {
    ai_client: Arc<A>,
    model: String,
}

impl<A: AIClient> ChatSummaryService<A> {
    pub fn new(ai_client: Arc<A>, model: &str) -> Self {
        Self {
            ai_client,
            model: model.to_string(),
        }
    }

    /// 对消息列表生成摘要
    pub async fn summarize(&self, messages: &[String]) -> XueliResult<String> {
        if messages.is_empty() {
            return Ok("（空对话）".to_string());
        }

        let conversation_text = messages.join("\n");

        let system_prompt = "你是一个对话摘要助手。请用简洁的中文概括以下对话的核心内容，\
            包括主要话题、关键信息和结论。摘要不超过200字。";

        let chat_messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: MessageContent::Text(system_prompt.to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: MessageContent::Text(format!("请概括以下对话：\n{}", conversation_text)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages: chat_messages,
            temperature: Some(0.3),
            max_tokens: Some(256),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = self.ai_client.chat_completion(&request).await?;

        debug!(
            msg_count = messages.len(),
            summary_len = response.content.len(),
            "[ChatSummary] 摘要生成完成"
        );

        Ok(response.content)
    }

    /// 简单规则摘要（无需 LLM 的降级方案）
    pub fn summarize_simple(messages: &[String]) -> String {
        if messages.is_empty() {
            return "（暂无对话）".to_string();
        }
        let count = messages.len();
        if count <= 2 {
            return format!("简短对话（{}条消息）", count);
        }
        let first = &messages[0];
        let last = &messages[messages.len() - 1];
        // 截取前80字符作为首条摘要
        let first_preview: String = first.chars().take(80).collect();
        let last_preview: String = last.chars().take(80).collect();
        format!(
            "共{}条消息，从「{}…」到「{}…」",
            count, first_preview, last_preview
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize_simple_empty() {
        let result =
            ChatSummaryService::<crate::services::ai_client::DefaultAIClient>::summarize_simple(&[]);
        assert!(result.contains("暂无"));
    }

    #[test]
    fn test_summarize_simple_short() {
        let msgs = vec!["你好".to_string(), "你好呀".to_string()];
        let result =
            ChatSummaryService::<crate::services::ai_client::DefaultAIClient>::summarize_simple(
                &msgs,
            );
        assert!(result.contains("2条消息"));
    }

    #[test]
    fn test_summarize_simple_long() {
        let msgs = vec![
            "今天天气很好，我们去公园散步吧。".to_string(),
            "好的，几点出发？".to_string(),
            "下午三点怎么样？".to_string(),
        ];
        let result =
            ChatSummaryService::<crate::services::ai_client::DefaultAIClient>::summarize_simple(
                &msgs,
            );
        assert!(result.contains("3条消息"));
    }
}
