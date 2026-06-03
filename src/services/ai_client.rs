use async_trait::async_trait;
use std::sync::Arc;

use crate::core::config::ModelConfig;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatCompletionResponse};

/// 默认 AI 客户端实现（基于 HTTP + OpenAI 兼容 API）
pub struct DefaultAIClient {
    config: Arc<ModelConfig>,
    client: reqwest::Client,
}

impl DefaultAIClient {
    pub fn new(config: Arc<ModelConfig>) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;
        Ok(Self { config, client })
    }
}

#[async_trait]
impl AIClient for DefaultAIClient {
    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, String> {
        let url = format!("{}/chat/completions", self.config.api_base);

        let body = serde_json::json!({
            "model": request.model,
            "messages": request.messages.iter().map(|m| serde_json::json!({
                "role": m.role,
                "content": m.content,
            })).collect::<Vec<_>>(),
            "temperature": request.temperature.unwrap_or(self.config.temperature),
            "max_tokens": request.max_tokens.unwrap_or(self.config.max_tokens),
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("AI API 请求失败: {}", e))?;

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("解析 AI 响应失败: {}", e))?;

        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let usage = json.get("usage").map(|u| {
            crate::traits::ai_client::TokenUsage {
                prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
                total_tokens: u["total_tokens"].as_u64().unwrap_or(0) as u32,
            }
        });

        Ok(ChatCompletionResponse {
            content,
            finish_reason: json["choices"][0]["finish_reason"]
                .as_str()
                .map(|s| s.to_string()),
            usage,
        })
    }
}