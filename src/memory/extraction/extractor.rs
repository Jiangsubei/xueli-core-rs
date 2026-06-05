use std::sync::Arc;
use std::time::Instant;

use crate::core::types::{MemoryItem, MemoryPatch, MemoryType};
use crate::prelude::XueliResult;
use crate::traits::ai_client::{
    AIClient, ChatCompletionRequest, ChatCompletionResponse, ChatMessage,
};

/// LLM 记忆提取器 — 从对话中提取结构化记忆
///
/// 对应 Python 版 `xueli/src/memory/extraction/extractor.py`
pub struct MemoryExtractor<A: AIClient> {
    ai_client: Arc<A>,
    model: String,
    max_retries: usize,
}

impl<A: AIClient> MemoryExtractor<A> {
    pub fn new(ai_client: Arc<A>, model: &str) -> Self {
        Self {
            ai_client,
            model: model.to_string(),
            max_retries: 3,
        }
    }

    /// 从一组消息中提取记忆
    pub async fn extract(&self, user_id: &str, messages: &[String]) -> XueliResult<MemoryPatch> {
        if messages.is_empty() {
            return Ok(MemoryPatch {
                add: Vec::new(),
                update: Vec::new(),
                remove: Vec::new(),
            });
        }

        let conversation = messages.join("\n");
        let system_prompt = self.build_system_prompt();
        let user_prompt = self.build_user_prompt(&conversation);

        let chat_messages = vec![
            ChatMessage::text("system", &system_prompt),
            ChatMessage::text("user", &user_prompt),
        ];

        let start = Instant::now();
        let mut last_err = String::new();

        for attempt in 0..self.max_retries {
            let request = ChatCompletionRequest {
                model: self.model.clone(),
                messages: chat_messages.clone(),
                temperature: Some(0.3),
                max_tokens: Some(1024),
                stream: false,
                extra_params: Default::default(),
            };

            match self.ai_client.chat_completion(&request).await {
                Ok(response) => match self.parse_response(&response, user_id) {
                    Ok(patch) => {
                        tracing::debug!(
                            user_id = user_id,
                            msg_count = messages.len(),
                            add_count = patch.add.len(),
                            elapsed_ms = start.elapsed().as_millis(),
                            "[MemoryExtractor] 提取完成"
                        );
                        return Ok(patch);
                    }
                    Err(e) => {
                        tracing::warn!(attempt = attempt, "[MemoryExtractor] 解析失败: {}", e);
                        last_err = e.to_string();
                    }
                },
                Err(e) => {
                    tracing::warn!(attempt = attempt, "[MemoryExtractor] AI 调用失败: {}", e);
                    last_err = e.to_string();
                }
            }
        }

        // 所有重试失败 → 返回空 patch（静默失败）
        tracing::warn!("[MemoryExtractor] 全部尝试失败，返回空记忆: {}", last_err);
        Ok(MemoryPatch {
            add: Vec::new(),
            update: Vec::new(),
            remove: Vec::new(),
        })
    }

    fn build_system_prompt(&self) -> String {
        r#"你是一个记忆提取助手。从对话中提取关于用户的有意义信息。

提取规则：
- 只提取关于用户的事实、偏好、经历或观点
- 每条记忆应该是一句简洁的陈述
- 记忆类型：fact（事实）、preference（偏好）、event（经历）、opinion（观点）、relationship（关系信息）
- 重要度 0.0-1.0：1.0 表示极其重要（如姓名、关键偏好），0.5 表示一般信息
- 如果没有值得记忆的内容，返回空列表

输出 JSON 格式：
```json
{
  "memories": [
    {
      "content": "记忆内容",
      "memory_type": "fact|preference|event|opinion|relationship",
      "importance": 0.8,
      "confidence": 0.9
    }
  ]
}
```

只输出 JSON，不要额外说明。"#
            .to_string()
    }

    fn build_user_prompt(&self, conversation: &str) -> String {
        format!(
            "请从以下对话中提取关于用户的值得记住的信息：\n\n```\n{}\n```\n\n请输出 JSON。",
            conversation
        )
    }

    fn parse_response(
        &self,
        response: &ChatCompletionResponse,
        user_id: &str,
    ) -> XueliResult<MemoryPatch> {
        let text = response.content.trim();
        if text.is_empty() {
            return Ok(MemoryPatch {
                add: Vec::new(),
                update: Vec::new(),
                remove: Vec::new(),
            });
        }

        // 提取 JSON
        let json_str = if let Some(start) = text.find('{') {
            let end = text.rfind('}').unwrap_or(text.len() - 1);
            &text[start..=end]
        } else {
            return Ok(MemoryPatch {
                add: Vec::new(),
                update: Vec::new(),
                remove: Vec::new(),
            });
        };

        let parsed: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| format!("JSON 解析失败: {e}"))?;

        let memories = match parsed.get("memories").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => {
                return Ok(MemoryPatch {
                    add: Vec::new(),
                    update: Vec::new(),
                    remove: Vec::new(),
                })
            }
        };

        let now = chrono::Utc::now();
        let items: Vec<MemoryItem> = memories
            .iter()
            .filter_map(|m| {
                let content = m.get("content")?.as_str()?.to_string();
                let importance = m.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.5);
                let confidence = m.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.5);

                // 低置信度记忆过滤
                if confidence < 0.3 {
                    return None;
                }

                let memory_type = m
                    .get("memory_type")
                    .and_then(|v| v.as_str())
                    .map(|s| match s {
                        "preference" => MemoryType::Preference,
                        "event" => MemoryType::Event,
                        "opinion" => MemoryType::Opinion,
                        "relationship" => MemoryType::Relationship,
                        _ => MemoryType::Fact,
                    })
                    .unwrap_or(MemoryType::Fact);

                Some(MemoryItem {
                    id: format!("mem_{}_{}", user_id, uuid::Uuid::new_v4().as_simple()),
                    user_id: user_id.to_string(),
                    content,
                    memory_type,
                    importance: importance.clamp(0.0, 1.0),
                    created_at: now,
                    last_accessed_at: now,
                    access_count: 0,
                })
            })
            .collect();

        Ok(MemoryPatch {
            add: items,
            update: Vec::new(),
            remove: Vec::new(),
        })
    }
}

impl<A: AIClient> Default for MemoryExtractor<A> {
    fn default() -> Self {
        // 需要提供有效的 AI 客户端，这里使用不可达的 panic
        unimplemented!("MemoryExtractor 需要 AI 客户端，请使用 new() 构造")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ai_client::NoopAIClient;
    use crate::traits::ai_client::ChatCompletionResponse;

    #[test]
    fn test_parse_response_empty() {
        let extractor = MemoryExtractor::new(Arc::new(NoopAIClient), "gpt-4o-mini");
        let response = ChatCompletionResponse {
            content: "".to_string(),
            reasoning_content: "".to_string(),
            finish_reason: "".to_string(),
            usage: None,
            model: "".to_string(),
            tool_calls: None,
            segments: None,
        };
        let patch = extractor.parse_response(&response, "u1").unwrap();
        assert!(patch.add.is_empty());
    }

    #[test]
    fn test_parse_response_with_memories() {
        let extractor = MemoryExtractor::new(Arc::new(NoopAIClient), "gpt-4o-mini");
        let json = r#"{
          "memories": [
            {"content": "用户喜欢喝咖啡", "memory_type": "preference", "importance": 0.7, "confidence": 0.9},
            {"content": "用户住在北京", "memory_type": "fact", "importance": 0.9, "confidence": 0.95}
          ]
        }"#;
        let response = ChatCompletionResponse {
            content: json.to_string(),
            reasoning_content: "".to_string(),
            finish_reason: "".to_string(),
            usage: None,
            model: "".to_string(),
            tool_calls: None,
            segments: None,
        };
        let patch = extractor.parse_response(&response, "u1").unwrap();
        assert_eq!(patch.add.len(), 2);
        assert!(patch.add[0].content.contains("咖啡"));
        assert_eq!(patch.add[1].memory_type, MemoryType::Fact);
        assert!(patch.add[1].importance > 0.8);
    }

    #[test]
    fn test_parse_response_filter_low_confidence() {
        let extractor = MemoryExtractor::new(Arc::new(NoopAIClient), "gpt-4o-mini");
        let json = r#"{
          "memories": [
            {"content": "可靠记忆", "memory_type": "fact", "importance": 0.9, "confidence": 0.95},
            {"content": "不确定的记忆", "memory_type": "fact", "importance": 0.5, "confidence": 0.2}
          ]
        }"#;
        let response = ChatCompletionResponse {
            content: json.to_string(),
            reasoning_content: "".to_string(),
            finish_reason: "".to_string(),
            usage: None,
            model: "".to_string(),
            tool_calls: None,
            segments: None,
        };
        let patch = extractor.parse_response(&response, "u1").unwrap();
        assert_eq!(patch.add.len(), 1);
        assert!(patch.add[0].content.contains("可靠"));
    }

    #[test]
    fn test_build_system_prompt_not_empty() {
        let extractor = MemoryExtractor::new(Arc::new(NoopAIClient), "gpt-4o-mini");
        let prompt = extractor.build_system_prompt();
        assert!(prompt.contains("记忆提取"));
        assert!(prompt.contains("JSON"));
    }
}
