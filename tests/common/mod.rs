use std::sync::Arc;
use xueli_core::core::config::XueliConfig;
use xueli_core::prelude::XueliResult;
use xueli_core::traits::ai_client::{
    AIClient, ChatCompletionRequest, ChatCompletionResponse,
};

/// 返回一个标准测试配置（所有功能开启，使用轻量模型）
pub fn test_config() -> XueliConfig {
    let mut config = XueliConfig::default();
    config.model.primary_model = "test-model".to_string();
    config.model.light_model = "test-model".to_string();
    config.emoji.enabled = true;
    config.proactive_share.enabled = false;
    config.drive.enabled = true;
    config
}

/// Mock AI 客户端 — 返回固定的 JSON 响应
pub struct MockAIClient {
    response: String,
}

impl MockAIClient {
    pub fn new(response: &str) -> Self {
        Self {
            response: response.to_string(),
        }
    }

    pub fn boxed(response: &str) -> Arc<dyn AIClient> {
        Arc::new(Self::new(response))
    }
}

#[async_trait::async_trait]
impl AIClient for MockAIClient {
    async fn chat_completion(
        &self,
        _request: &ChatCompletionRequest,
    ) -> XueliResult<ChatCompletionResponse> {
        Ok(ChatCompletionResponse {
            content: self.response.clone(),
            segments: None,
            reasoning_content: String::new(),
            finish_reason: "stop".to_string(),
            usage: None,
            model: "test-model".to_string(),
            tool_calls: None,
            raw_response: None,
            raw_content: self.response.clone(),
        })
    }
}

#[test]
fn common_module_loaded() {}
