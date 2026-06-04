use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Semaphore;

use crate::prelude::XueliResult;
use crate::core::config::ModelConfig;
use crate::traits::ai_client::{
    AIClient, ChatCompletionRequest, ChatCompletionResponse, FunctionCall, TokenUsage, ToolCall,
};

/// 默认 AI 客户端实现（基于 HTTP + OpenAI 兼容 API）
///
/// 对应 Python 版 `xueli/src/services/ai_client.py` + `services/ai/` 子模块
pub struct DefaultAIClient {
    config: Arc<ModelConfig>,
    client: reqwest::Client,
    semaphore: Semaphore,
}

impl DefaultAIClient {
    pub fn new(config: Arc<ModelConfig>) -> XueliResult<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        if !config.api_key.is_empty() {
            let mut auth_value =
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", config.api_key))
                    .map_err(|e| format!("无效 API Key: {}", e))?;
            auth_value.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, auth_value);
        }
        for (key, value) in &config.extra_headers {
            headers.insert(
                reqwest::header::HeaderName::from_bytes(key.as_bytes())
                    .map_err(|e| format!("无效请求头名 '{}': {}", key, e))?,
                reqwest::header::HeaderValue::from_str(value)
                    .map_err(|e| format!("无效请求头值 '{}': {}", key, e))?,
            );
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(config.timeout as u64))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

        Ok(Self {
            config,
            client,
            semaphore: Semaphore::new(5), // 与 Python 版一致
        })
    }

    /// 构建请求体
    fn build_request_body(
        &self,
        request: &ChatCompletionRequest,
    ) -> XueliResult<serde_json::Value> {
        let mut body = serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "stream": request.stream,
        });

        if let Some(temperature) = request.temperature {
            body["temperature"] = serde_json::json!(temperature);
        }
        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }

        // 合并 config 级别 extra_params
        for (key, value) in &self.config.extra_params {
            body[key] = value.clone();
        }
        // 合并 request 级别 extra_params（优先级更高）
        for (key, value) in &request.extra_params {
            body[key] = value.clone();
        }

        Ok(body)
    }

    /// 核心请求（不含重试）
    async fn do_chat_completion(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> XueliResult<(u16, String)> {
        let response = self
            .client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|e| format!("HTTP 请求失败: {}", e))?;

        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        Ok((status, text))
    }

    /// 多路径提取响应内容
    fn extract_content(json: &serde_json::Value, response_path: &str) -> String {
        // 1. 先尝试配置的 response_path
        if let Some(content) = resolve_json_path(json, response_path) {
            return stringify_json_value(content);
        }

        // 2. 回退路径列表
        let fallback_paths = [
            "choices.0.message.content",
            "output.choices.0.message.content",
            "choices.0.message.reasoning_content",
            "choices.0.text",
            "output.text",
            "text",
            "content",
            "output_text",
        ];

        for path in &fallback_paths {
            if path == &response_path {
                continue; // 避免重复
            }
            if let Some(content) = resolve_json_path(json, path) {
                return stringify_json_value(content);
            }
        }

        String::new()
    }

    /// 提取 tool_calls
    fn extract_tool_calls(json: &serde_json::Value) -> Option<Vec<ToolCall>> {
        let candidate_paths = [
            "choices.0.message.tool_calls",
            "output.choices.0.message.tool_calls",
        ];

        for path in &candidate_paths {
            if let Some(tc) = resolve_json_path(json, path) {
                if let Some(arr) = tc.as_array() {
                    let calls: Vec<ToolCall> = arr
                        .iter()
                        .filter_map(|item| {
                            Some(ToolCall {
                                id: item
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                call_type: item
                                    .get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("function")
                                    .to_string(),
                                function: FunctionCall {
                                    name: item
                                        .get("function")
                                        .and_then(|f| f.get("name"))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    arguments: item
                                        .get("function")
                                        .and_then(|f| f.get("arguments"))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                },
                            })
                        })
                        .collect();
                    if !calls.is_empty() {
                        return Some(calls);
                    }
                }
            }
        }
        None
    }

    /// 解析响应为归一化结构
    fn parse_response(
        json: &serde_json::Value,
        response_path: &str,
        default_model: &str,
    ) -> ChatCompletionResponse {
        let content = Self::extract_content(json, response_path);
        let reasoning_content = resolve_json_path(json, "choices.0.message.reasoning_content")
            .map(|v| stringify_json_value(v))
            .unwrap_or_default();
        let finish_reason = resolve_json_path(json, "choices.0.finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let model = json
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or(default_model)
            .to_string();
        let usage = json.get("usage").map(|u| TokenUsage {
            prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            completion_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        });
        let tool_calls = Self::extract_tool_calls(json);

        ChatCompletionResponse {
            content,
            segments: None,
            reasoning_content,
            finish_reason,
            usage,
            model,
            tool_calls,
        }
    }
}

#[async_trait]
impl AIClient for DefaultAIClient {
    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> XueliResult<ChatCompletionResponse> {
        let url = format!(
            "{}/chat/completions",
            self.config.api_base.trim_end_matches('/')
        );
        let body = self.build_request_body(request)?;
        let max_retries = self.config.max_retries;

        // 并发限制
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| format!("获取并发许可失败: {}", e))?;

        for attempt in 0..=max_retries {
            let (status, response_text) = match self.do_chat_completion(&url, &body).await {
                Ok(result) => result,
                Err(e) => {
                    // 网络层错误 → 重试
                    if attempt < max_retries {
                        let delay = 2_u64.pow(attempt as u32);
                        tracing::warn!(
                            "[AI客户端] 第 {}/{max_retries} 次重试（网络错误），等待 {delay}s",
                            attempt + 1,
                        );
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                        continue;
                    }
                    return Err(format!("网络请求失败（{} 次重试后）: {}", max_retries, e).into());
                }
            };

            // 可重试的 HTTP 状态码（429 / 5xx）
            if status == 429 || status >= 500 {
                if attempt < max_retries {
                    let delay = 2_u64.pow(attempt as u32);
                    tracing::warn!(
                        "[AI客户端] HTTP {status} — 第 {}/{max_retries} 次重试，等待 {delay}s",
                        attempt + 1,
                    );
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    continue;
                }
                return Err(format!(
                    "AI API 请求失败（{} 次重试后）: HTTP {}",
                    max_retries, status
                ).into());
            }

            // 非 200 → 不重试
            if status != 200 {
                let preview = &response_text[..response_text.len().min(500)];
                return Err(format!("AI API 请求失败: HTTP {}, {}", status, preview).into());
            }

            // 解析 JSON
            let json: serde_json::Value = match serde_json::from_str(&response_text) {
                Ok(v) => v,
                Err(e) => {
                    if attempt < max_retries {
                        let delay = 2_u64.pow(attempt as u32);
                        tracing::warn!(
                            "[AI客户端] JSON 解析失败 — 第 {}/{max_retries} 次重试，等待 {delay}s",
                            attempt + 1,
                        );
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                        continue;
                    }
                    return Err(format!(
                        "无法解析 AI 响应（{} 次重试后）: {}",
                        max_retries, e
                    ).into());
                }
            };

            return Ok(Self::parse_response(
                &json,
                &self.config.response_path,
                &request.model,
            ));
        }

        Err(format!("AI API 请求失败（{} 次重试后）", max_retries).into())
    }
}

// ── 工具函数 ──────────────────────────────────────────────

/// 按点分隔路径解析 JSON 值
fn resolve_json_path<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for part in path.split('.') {
        match current {
            serde_json::Value::Array(arr) => {
                let index: usize = part.parse().ok()?;
                current = arr.get(index)?;
            }
            serde_json::Value::Object(map) => {
                current = map.get(part)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// 将 JSON 值转为字符串
fn stringify_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr
                .iter()
                .map(|item| match item {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(o) => o
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    _ => item.to_string(),
                })
                .filter(|s| !s.is_empty())
                .collect();
            parts.join("\n")
        }
        _ => value.to_string(),
    }
}

/// Noop AI 客户端 — 用于测试和降级场景
pub struct NoopAIClient;

#[async_trait]
impl AIClient for NoopAIClient {
    async fn chat_completion(
        &self,
        _request: &ChatCompletionRequest,
    ) -> XueliResult<ChatCompletionResponse> {
        Err("NoopAIClient: 未配置 AI 客户端".to_string().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ai_client::{ChatMessage, ContentPart, ImageUrlPayload, MessageContent};
    use serde_json::json;

    #[test]
    fn test_extract_content_with_response_path() {
        let json = json!({
            "choices": [{
                "message": {
                    "content": "你好！"
                }
            }]
        });
        let result = DefaultAIClient::extract_content(&json, "choices.0.message.content");
        assert_eq!(result, "你好！");
    }

    #[test]
    fn test_extract_content_fallback() {
        let json = json!({
            "output": {
                "text": "回退路径测试"
            }
        });
        let result = DefaultAIClient::extract_content(&json, "nonexistent.path");
        assert_eq!(result, "回退路径测试");
    }

    #[test]
    fn test_extract_content_empty() {
        let json = json!({});
        let result = DefaultAIClient::extract_content(&json, "nonexistent");
        assert_eq!(result, "");
    }

    #[test]
    fn test_extract_tool_calls() {
        let json = json!({
            "choices": [{
                "message": {
                    "content": "我来帮你查天气",
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\": \"北京\"}"
                        }
                    }]
                }
            }]
        });
        let result = DefaultAIClient::extract_tool_calls(&json);
        assert!(result.is_some());
        let calls = result.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_123");
        assert_eq!(calls[0].function.name, "get_weather");
    }

    #[test]
    fn test_parse_response_full() {
        let json = json!({
            "model": "gpt-4o",
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "content": "回复内容",
                    "reasoning_content": "推理过程..."
                }
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "total_tokens": 150
            }
        });
        let response =
            DefaultAIClient::parse_response(&json, "choices.0.message.content", "default-model");
        assert_eq!(response.content, "回复内容");
        assert_eq!(response.reasoning_content, "推理过程...");
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.model, "gpt-4o");
        assert!(response.usage.is_some());
    }

    #[test]
    fn test_resolve_json_path_nested() {
        let json = json!({"a": {"b": {"c": "value"}}});
        let result = resolve_json_path(&json, "a.b.c");
        assert_eq!(result.and_then(|v| v.as_str()), Some("value"));
    }

    #[test]
    fn test_resolve_json_path_array_index() {
        let json = json!({"items": [{"name": "first"}, {"name": "second"}]});
        let result = resolve_json_path(&json, "items.1.name");
        assert_eq!(result.and_then(|v| v.as_str()), Some("second"));
    }

    #[test]
    fn test_resolve_json_path_not_found() {
        let json = json!({"a": 1});
        let result = resolve_json_path(&json, "a.b.c");
        assert!(result.is_none());
    }

    #[test]
    fn test_stringify_json_value_string() {
        assert_eq!(stringify_json_value(&json!("hello")), "hello");
    }

    #[test]
    fn test_stringify_json_value_array_of_strings() {
        let result = stringify_json_value(&json!(["part1", "part2"]));
        assert_eq!(result, "part1\npart2");
    }

    #[test]
    fn test_stringify_json_value_array_with_objects() {
        let result = stringify_json_value(&json!([{"text": "hello"}, {"text": "world"}]));
        assert_eq!(result, "hello\nworld");
    }

    #[test]
    fn test_message_content_text_serialization() {
        let content = MessageContent::Text("hello".to_string());
        let serialized = serde_json::to_string(&content).unwrap();
        assert_eq!(serialized, r#""hello""#);

        let deserialized: MessageContent = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.text(), "hello");
    }

    #[test]
    fn test_message_content_multimodal_serialization() {
        let content = MessageContent::Multimodal(vec![
            ContentPart::Text {
                text: "这是什么？".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrlPayload {
                    url: "data:image/jpeg;base64,abc123".to_string(),
                    detail: None,
                },
            },
        ]);
        let serialized = serde_json::to_string(&content).unwrap();
        // 应序列化为数组
        assert!(serialized.starts_with('['));
        assert!(serialized.contains("image_url"));

        let deserialized: MessageContent = serde_json::from_str(&serialized).unwrap();
        assert!(matches!(deserialized, MessageContent::Multimodal(_)));
    }

    #[test]
    fn test_chat_message_multimodal_builder() {
        let msg =
            ChatMessage::multimodal("user", "描述这张图", &["abc123".to_string()], "image/jpeg");
        assert_eq!(msg.role, "user");
        assert!(msg.content.has_images());
        assert_eq!(msg.content.text(), "描述这张图");
    }
}
