use std::sync::Arc;

use crate::core::platform_types::InboundEvent;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};

/// 回复代理 — 调用 AI 生成本次回复
pub struct ReplyAgent<A: AIClient> {
    ai_client: Arc<A>,
}

impl<A: AIClient> ReplyAgent<A> {
    pub fn new(ai_client: Arc<A>) -> Self {
        Self { ai_client }
    }

    /// 生成回复文本
    pub async fn generate_reply(&self, event: &InboundEvent) -> Result<String, String> {
        let user_text = event
            .message
            .as_ref()
            .map(|m| m.text.clone())
            .unwrap_or_default();

        let sender_name = event
            .message
            .as_ref()
            .map(|m| m.sender_name.clone())
            .unwrap_or_else(|| "用户".to_string());

        let system_prompt = format!(
            "你是 {} 的好友，请根据对话内容给出自然、亲切的回复。",
            sender_name
        );

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt,
            },
            ChatMessage {
                role: "user".to_string(),
                content: user_text,
            },
        ];

        let request = ChatCompletionRequest {
            model: "gpt-4o-mini".to_string(),
            messages,
            temperature: Some(0.8),
            max_tokens: Some(512),
        };

        let response = self.ai_client.chat_completion(&request).await?;
        Ok(response.content)
    }
}