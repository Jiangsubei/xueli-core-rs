use async_trait::async_trait;
use std::sync::Arc;

use crate::core::config::ModelConfig;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatCompletionResponse, ChatMessage};

/// VLM 视觉客户端 — 图片理解
pub struct VisionClient {
    config: Arc<ModelConfig>,
    client: Arc<dyn AIClient>,
}

impl VisionClient {
    pub fn new(config: Arc<ModelConfig>, client: Arc<dyn AIClient>) -> Self {
        Self { config, client }
    }

    /// 分析图片内容
    pub async fn analyze_image(
        &self,
        image_base64: &str,
        prompt: &str,
        mime_type: &str,
    ) -> Result<String, String> {
        let model = self
            .config
            .vision_model
            .as_ref()
            .ok_or("未配置 VLM 模型")?
            .clone();

        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: format!("[image:{}] {}", mime_type, prompt),
        }];

        let request = ChatCompletionRequest {
            model,
            messages,
            temperature: Some(0.3),
            max_tokens: Some(1024),
        };

        let response = self.client.chat_completion(&request).await?;
        Ok(response.content)
    }
}