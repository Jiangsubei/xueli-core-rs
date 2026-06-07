use std::sync::Arc;
use std::time::Instant;

use crate::core::types::MemoryItem;
use crate::prelude::XueliResult;
use crate::traits::ai_client::{
    AIClient, ChatCompletionRequest, ChatCompletionResponse, ChatMessage,
};

/// 记忆冲突反思 — 检测新旧记忆矛盾并给出解决方案
///
/// 对应 Python 版 `xueli/src/memory/extraction/reflection.py`
pub struct MemoryReflection<A: AIClient> {
    ai_client: Arc<A>,
    model: String,
    max_retries: usize,
}

impl<A: AIClient> MemoryReflection<A> {
    pub fn new(ai_client: Arc<A>, model: &str) -> Self {
        Self {
            ai_client,
            model: model.to_string(),
            max_retries: 3,
        }
    }

    pub async fn reflect(
        &self,
        existing: &[MemoryItem],
        new_items: &[MemoryItem],
    ) -> XueliResult<ReflectionResult> {
        if existing.is_empty() || new_items.is_empty() {
            return Ok(ReflectionResult {
                conflicts: Vec::new(),
                resolutions: Vec::new(),
            });
        }

        let system_prompt = self.build_system_prompt();
        let user_prompt = self.build_user_prompt(existing, new_items);

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
                temperature: Some(0.2),
                max_tokens: Some(512),
                stream: false,
                tools: None,
                tool_choice: None,
                extra_params: Default::default(),
            };

            match self.ai_client.chat_completion(&request).await {
                Ok(response) => match self.parse_response(&response) {
                    Ok(result) => {
                        tracing::debug!(
                            conflict_count = result.conflicts.len(),
                            elapsed_ms = start.elapsed().as_millis(),
                            "[MemoryReflection] 反思完成"
                        );
                        return Ok(result);
                    }
                    Err(e) => {
                        tracing::warn!(attempt = attempt, "[MemoryReflection] 解析失败: {}", e);
                        last_err = e.to_string();
                    }
                },
                Err(e) => {
                    tracing::warn!(attempt = attempt, "[MemoryReflection] AI 调用失败: {}", e);
                    last_err = e.to_string();
                }
            }
        }

        tracing::warn!("[MemoryReflection] 全部尝试失败，返回空反思: {}", last_err);
        Ok(ReflectionResult {
            conflicts: Vec::new(),
            resolutions: Vec::new(),
        })
    }

    fn build_system_prompt(&self) -> String {
        r#"你是一个记忆反思助手。检测新旧记忆之间的冲突并给出解决方案。

规则：
- 如果两条记忆信息矛盾，标记为冲突
- 若新信息纠正了旧信息，建议「更新旧记忆」
- 若两条记忆可以共存（如不同时间点的状态），建议「保留两者」
- 如果没有冲突，返回空列表

输出 JSON 格式：
```json
{
  "conflicts": [
    {
      "old_memory": "旧记忆内容",
      "new_memory": "新记忆内容",
      "type": "contradiction|update|complement",
      "resolution": "keep_both|update_old|replace_old",
      "reason": "推理理由"
    }
  ]
}
```

只输出 JSON，不要额外说明。"#
            .to_string()
    }

    fn build_user_prompt(&self, existing: &[MemoryItem], new_items: &[MemoryItem]) -> String {
        let old_list: Vec<String> = existing
            .iter()
            .map(|m| format!("- [{}] {}", m.id, m.content))
            .collect();
        let new_list: Vec<String> = new_items
            .iter()
            .map(|m| format!("- [{}] {}", m.id, m.content))
            .collect();

        format!(
            "请检查以下新旧记忆之间是否有冲突：\n\n【已有记忆】\n{}\n\n【新记忆】\n{}\n\n请输出 JSON。",
            old_list.join("\n"),
            new_list.join("\n"),
        )
    }

    fn parse_response(&self, response: &ChatCompletionResponse) -> XueliResult<ReflectionResult> {
        let text = response.content.trim();
        if text.is_empty() {
            return Ok(ReflectionResult {
                conflicts: Vec::new(),
                resolutions: Vec::new(),
            });
        }

        let json_str = if let Some(start) = text.find('{') {
            let end = text.rfind('}').unwrap_or(text.len() - 1);
            &text[start..=end]
        } else {
            return Ok(ReflectionResult {
                conflicts: Vec::new(),
                resolutions: Vec::new(),
            });
        };

        let parsed: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| format!("JSON 解析失败: {e}"))?;

        let conflicts: Vec<String> = parsed
            .get("conflicts")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|c| {
                        let reason = c.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                        let resolution = c
                            .get("resolution")
                            .and_then(|v| v.as_str())
                            .unwrap_or("keep_both");
                        format!("[{}] {}", resolution, reason)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let resolutions: Vec<String> = conflicts.clone();

        Ok(ReflectionResult {
            conflicts,
            resolutions,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ReflectionResult {
    pub conflicts: Vec<String>,
    pub resolutions: Vec<String>,
}

impl<A: AIClient> Default for MemoryReflection<A> {
    fn default() -> Self {
        unimplemented!("MemoryReflection 需要 AI 客户端，请使用 new() 构造")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ai_client::NoopAIClient;
    use crate::traits::ai_client::ChatCompletionResponse;

    #[test]
    fn test_parse_response_with_conflicts() {
        let reflection = MemoryReflection::new(Arc::new(NoopAIClient), "gpt-4o-mini");
        let json = r#"{
          "conflicts": [
            {
              "old_memory": "用户住在上海",
              "new_memory": "用户搬家到北京",
              "type": "update",
              "resolution": "update_old",
              "reason": "位置信息已更新"
            }
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
        let result = reflection.parse_response(&response).unwrap();
        assert_eq!(result.conflicts.len(), 1);
        assert!(result.conflicts[0].contains("update_old"));
    }

    #[test]
    fn test_parse_response_no_conflicts() {
        let reflection = MemoryReflection::new(Arc::new(NoopAIClient), "gpt-4o-mini");
        let json = r#"{"conflicts": []}"#;
        let response = ChatCompletionResponse {
            content: json.to_string(),
            reasoning_content: "".to_string(),
            finish_reason: "".to_string(),
            usage: None,
            model: "".to_string(),
            tool_calls: None,
            segments: None,
        };
        let result = reflection.parse_response(&response).unwrap();
        assert!(result.conflicts.is_empty());
    }
}
