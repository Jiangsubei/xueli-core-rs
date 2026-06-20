use crate::prelude::XueliResult;
use crate::traits::ai_client::ToolCall;
use crate::traits::tool_calling::{ToolCallingStrategy, ToolDefinition};

/// OpenAI function calling 格式的默认策略
#[derive(Debug, Clone, Default)]
pub struct OpenAIToolCallingStrategy;

impl OpenAIToolCallingStrategy {
    /// 创建新的 OpenAI 工具调用策略
    pub fn new() -> Self {
        Self
    }

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

    /// 从已解析的 JSON 值中提取 tool_calls（与 DefaultAIClient 逻辑保持一致）
    fn extract_tool_calls_from_json(json: &serde_json::Value) -> Option<Vec<ToolCall>> {
        let candidate_paths = [
            "choices.0.message.tool_calls",
            "output.choices.0.message.tool_calls",
            "choices.0.message.functions",
            "output.choices.0.message.functions",
        ];

        for path in &candidate_paths {
            if let Some(tc) = Self::resolve_json_path(json, path) {
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
                                function: crate::traits::ai_client::FunctionCall {
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
}

impl ToolCallingStrategy for OpenAIToolCallingStrategy {
    fn parse_tool_calls(&self, response_text: &str) -> XueliResult<Vec<ToolCall>> {
        if response_text.is_empty() {
            return Ok(Vec::new());
        }
        let json: serde_json::Value = serde_json::from_str(response_text)?;
        Ok(Self::extract_tool_calls_from_json(&json).unwrap_or_default())
    }

    fn serialize_tools(&self, tools: &[ToolDefinition]) -> XueliResult<serde_json::Value> {
        let serialized: Vec<serde_json::Value> = tools
            .iter()
            .map(|def| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": def.name,
                        "description": def.description,
                        "parameters": def.parameters,
                    }
                })
            })
            .collect();
        Ok(serialized.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_serialize_tools_matches_manual_format() {
        let strategy = OpenAIToolCallingStrategy::new();
        let tools = vec![ToolDefinition {
            name: "reply".to_string(),
            description: "发送回复".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            }),
        }];

        let serialized = strategy.serialize_tools(&tools).unwrap();
        let arr = serialized.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "function");
        assert_eq!(arr[0]["function"]["name"], "reply");
        assert!(arr[0]["function"]["parameters"]["properties"]["text"].is_object());
    }

    #[test]
    fn test_parse_tool_calls_from_openai_response() {
        let strategy = OpenAIToolCallingStrategy::new();
        let response = json!({
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

        let calls = strategy.parse_tool_calls(&response.to_string()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_123");
        assert_eq!(calls[0].function.name, "get_weather");
        assert_eq!(calls[0].function.arguments, "{\"city\": \"北京\"}");
    }

    #[test]
    fn test_parse_tool_calls_empty_response() {
        let strategy = OpenAIToolCallingStrategy::new();
        let calls = strategy.parse_tool_calls("").unwrap();
        assert!(calls.is_empty());
    }
}
