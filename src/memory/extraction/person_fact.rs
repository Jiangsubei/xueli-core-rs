use std::sync::Arc;
use std::time::Instant;

use crate::memory::stores::person_fact::PersonFact;
use crate::prelude::XueliResult;
use crate::traits::ai_client::{
    AIClient, ChatCompletionRequest, ChatCompletionResponse, ChatMessage,
};

/// 人物事实服务 — 从对话中提取关于用户的人物事实
///
/// 对应 Python 版人物事实记忆提取逻辑
pub struct PersonFactService<A: AIClient> {
    ai_client: Arc<A>,
    model: String,
    max_retries: usize,
}

impl<A: AIClient> PersonFactService<A> {
    pub fn new(ai_client: Arc<A>, model: &str) -> Self {
        Self {
            ai_client,
            model: model.to_string(),
            max_retries: 3,
        }
    }

    /// 从对话中提取人物事实
    pub async fn extract_facts(
        &self,
        user_id: &str,
        messages: &[String],
    ) -> XueliResult<Vec<PersonFact>> {
        if messages.is_empty() {
            return Ok(Vec::new());
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
                tools: None,
                tool_choice: None,
                extra_params: Default::default(),
            };

            match self.ai_client.chat_completion(&request).await {
                Ok(response) => match self.parse_response(&response, user_id) {
                    Ok(facts) => {
                        tracing::debug!(
                            user_id = user_id,
                            fact_count = facts.len(),
                            elapsed_ms = start.elapsed().as_millis(),
                            "[PersonFact] 提取完成"
                        );
                        return Ok(facts);
                    }
                    Err(e) => {
                        tracing::warn!(attempt = attempt, "[PersonFact] 解析失败: {}", e);
                        last_err = e.to_string();
                    }
                },
                Err(e) => {
                    tracing::warn!(attempt = attempt, "[PersonFact] AI 调用失败: {}", e);
                    last_err = e.to_string();
                }
            }
        }

        // 静默失败
        tracing::warn!("[PersonFact] 全部尝试失败，返回空事实: {}", last_err);
        Ok(Vec::new())
    }

    fn build_system_prompt(&self) -> String {
        r#"你是一个人物画像提取助手。从对话中提取关于用户的个人特征和背景信息。

提取规则：
- 只提取关于用户的稳定个人特征（不会频繁改变的信息）
- 类别：name（姓名）、occupation（职业）、hobby（爱好）、location（位置）、skill（技能）、habit（习惯）、other（其他）
- 如果信息不明确或可能是临时的，不要提取
- 如果没有值得提取的信息，返回空列表

输出 JSON 格式：
```json
{
  "facts": [
    {
      "content": "事实描述",
      "category": "name|occupation|hobby|location|skill|habit|other",
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
            "请从以下对话中提取关于用户的个人特征信息：\n\n```\n{}\n```\n\n请输出 JSON。",
            conversation
        )
    }

    fn parse_response(
        &self,
        response: &ChatCompletionResponse,
        user_id: &str,
    ) -> XueliResult<Vec<PersonFact>> {
        let text = response.content.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }

        let json_str = if let Some(start) = text.find('{') {
            let end = text.rfind('}').unwrap_or(text.len() - 1);
            &text[start..=end]
        } else {
            return Ok(Vec::new());
        };

        let parsed: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| format!("JSON 解析失败: {e}"))?;

        let facts = match parsed.get("facts").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => return Ok(Vec::new()),
        };

        let now = chrono::Utc::now();
        let person_facts: Vec<PersonFact> = facts
            .iter()
            .filter_map(|f| {
                let content = f.get("content")?.as_str()?.to_string();
                let category = f
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("other")
                    .to_string();
                let confidence = f.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.5);

                if confidence < 0.3 {
                    return None;
                }

                Some(PersonFact {
                    id: format!("pf_{}_{}", user_id, uuid::Uuid::new_v4().as_simple()),
                    user_id: user_id.to_string(),
                    fact_text: content,
                    category,
                    source_conversation_id: None,
                    confidence,
                    created_at: now,
                    updated_at: now,
                })
            })
            .collect();

        Ok(person_facts)
    }
}

impl<A: AIClient> Default for PersonFactService<A> {
    fn default() -> Self {
        unimplemented!("PersonFactService 需要 AI 客户端，请使用 new() 构造")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ai_client::NoopAIClient;
    use crate::traits::ai_client::ChatCompletionResponse;

    #[test]
    fn test_parse_response_with_facts() {
        let service = PersonFactService::new(Arc::new(NoopAIClient), "gpt-4o-mini");
        let json = r#"{
          "facts": [
            {"content": "用户是程序员", "category": "occupation", "confidence": 0.95},
            {"content": "用户喜欢打篮球", "category": "hobby", "confidence": 0.8}
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
        let facts = service.parse_response(&response, "u1").unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].category, "occupation");
        assert!(facts[0].fact_text.contains("程序员"));
    }

    #[test]
    fn test_parse_response_empty() {
        let service = PersonFactService::new(Arc::new(NoopAIClient), "gpt-4o-mini");
        let response = ChatCompletionResponse {
            content: "".to_string(),
            reasoning_content: "".to_string(),
            finish_reason: "".to_string(),
            usage: None,
            model: "".to_string(),
            tool_calls: None,
            segments: None,
        };
        let facts = service.parse_response(&response, "u1").unwrap();
        assert!(facts.is_empty());
    }

    #[test]
    fn test_parse_response_filter_low_confidence() {
        let service = PersonFactService::new(Arc::new(NoopAIClient), "gpt-4o-mini");
        let json = r#"{
          "facts": [
            {"content": "可靠信息", "category": "fact", "confidence": 0.95},
            {"content": "不确定", "category": "fact", "confidence": 0.1}
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
        let facts = service.parse_response(&response, "u1").unwrap();
        assert_eq!(facts.len(), 1);
    }
}
