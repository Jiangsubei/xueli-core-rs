use std::sync::Arc;

use crate::prelude::XueliResult;
use crate::core::platform_types::InboundEvent;
use crate::core::types::ReplyPlan;
use crate::handlers::context_builder::ConversationContext;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};

/// 会话规划器 — 调用 LLM 规划回复策略
pub struct ConversationPlanner<A: AIClient> {
    ai_client: Arc<A>,
    /// 使用的模型名称
    model: String,
}

/// 规划结果
#[derive(Debug, Clone)]
pub struct PlanResult {
    /// 回复计划
    pub reply_plan: ReplyPlan,
    /// 提示词计划（下游 ReplyAgent 使用）
    pub prompt_plan: Option<PromptPlan>,
    /// 回复参考文本
    pub reply_reference: String,
    /// 是否应该回复
    pub should_reply: bool,
    /// 置信度
    pub confidence: f64,
}

/// 提示词计划 — 指导 ReplyAgent 如何构建回复
#[derive(Debug, Clone)]
pub struct PromptPlan {
    /// 回复风格
    pub tone_profile: Option<String>,
    /// 回复长度偏好
    pub length_preference: Option<String>,
    /// 主动性
    pub initiative: Option<String>,
    /// 上下文焦点
    pub context_focus: Option<String>,
    /// 需避免的话题
    pub avoid_topics: Vec<String>,
}

impl<A: AIClient> ConversationPlanner<A> {
    pub fn new(ai_client: Arc<A>, model: &str) -> Self {
        Self {
            ai_client,
            model: model.to_string(),
        }
    }

    /// 规划回复 — 调用 LLM 生成 ReplyPlan
    pub async fn plan(
        &self,
        event: &InboundEvent,
        context: &ConversationContext,
    ) -> XueliResult<PlanResult> {
        let user_message = context.user_message.clone();
        let is_group = context.is_group;

        let system_prompt = self.build_system_prompt(is_group);

        // 构建近期对话历史文本
        let history_text = if context.recent_messages.is_empty() {
            "（无近期对话记录）".to_string()
        } else {
            context.recent_messages.join("\n")
        };

        let messages = vec![
            ChatMessage::text("system", system_prompt),
            ChatMessage::text(
                "user",
                format!(
                    "近期对话记录：\n{}\n\n当前用户消息：{}",
                    history_text, user_message
                ),
            ),
        ];

        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(0.3),
            max_tokens: Some(1024),
            stream: false,
            extra_params: Default::default(),
        };

        let response = self.ai_client.chat_completion(&request).await?;
        let content = response.content.clone();

        // 尝试解析 JSON 响应
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(json) => {
                let reply_plan = ReplyPlan {
                    id: uuid::Uuid::new_v4().to_string(),
                    target_message_id: event
                        .message
                        .as_ref()
                        .map(|m| m.id.clone())
                        .unwrap_or_default(),
                    topic: json
                        .pointer("/reply_reference")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    style: json
                        .pointer("/prompt_plan/tone_profile")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    memory_recall_needed: false,
                    use_emoji: true,
                    priority: 0,
                };

                let prompt_plan = parse_prompt_plan(&json);
                let reply_reference = extract_str(&json, "/reply_reference").unwrap_or_default();

                Ok(PlanResult {
                    reply_plan,
                    prompt_plan,
                    reply_reference,
                    should_reply: true,
                    confidence: 0.8,
                })
            }
            Err(_) => {
                // JSON 解析失败，回退为默认计划
                let fallback_content = content.clone();
                Ok(PlanResult {
                    reply_plan: ReplyPlan {
                        id: uuid::Uuid::new_v4().to_string(),
                        target_message_id: event
                            .message
                            .as_ref()
                            .map(|m| m.id.clone())
                            .unwrap_or_default(),
                        topic: Some(content),
                        style: None,
                        memory_recall_needed: false,
                        use_emoji: true,
                        priority: 0,
                    },
                    prompt_plan: None,
                    reply_reference: fallback_content,
                    should_reply: true,
                    confidence: 0.5,
                })
            }
        }
    }

    /// 构建 system prompt
    fn build_system_prompt(&self, is_group: bool) -> String {
        let chat_mode = if is_group { "群聊" } else { "私聊" };

        format!(
            r#"你是会话控制中枢，负责在{chat_mode}场景下处理本轮对话。

上游已决定回复，你输出本轮回复所需的全部规划与决策。

【你要输出的内容】
1. reply_reference — 回复方向参考（自然语言短句，给下游回复模型看的方向提示）
2. prompt_plan — 回复上下文策略
   - tone_profile: 语气风格（如"轻松亲切"、"正式专业"、"幽默调侃"）
   - length_preference: 回复长度偏好（如"短"、"中等"、"长"）
   - initiative: 主动性（如"接话"、"主动提问"、"被动回应"）
   - context_focus: 上下文焦点（如"当前消息"、"近期对话"、"用户画像"）
   - avoid_topics: 需避免的话题列表

【输出格式】
你必须只输出 JSON，格式如下：
{{
  "reply_reference": "回复方向简述",
  "prompt_plan": {{
    "tone_profile": "轻松亲切",
    "length_preference": "短",
    "initiative": "接话",
    "context_focus": "当前消息",
    "avoid_topics": []
  }}
}}
不要输出 JSON 以外的任何内容。"#
        )
    }
}

fn parse_prompt_plan(json: &serde_json::Value) -> Option<PromptPlan> {
    let pp = json.get("prompt_plan")?;
    Some(PromptPlan {
        tone_profile: pp
            .get("tone_profile")
            .and_then(|v| v.as_str())
            .map(String::from),
        length_preference: pp
            .get("length_preference")
            .and_then(|v| v.as_str())
            .map(String::from),
        initiative: pp
            .get("initiative")
            .and_then(|v| v.as_str())
            .map(String::from),
        context_focus: pp
            .get("context_focus")
            .and_then(|v| v.as_str())
            .map(String::from),
        avoid_topics: pp
            .get("avoid_topics")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn extract_str(json: &serde_json::Value, pointer: &str) -> Option<String> {
    json.pointer(pointer)
        .and_then(|v| v.as_str())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_prompt_plan_full() {
        let json = serde_json::json!({
            "reply_reference": "简短回复用户问好",
            "prompt_plan": {
                "tone_profile": "轻松亲切",
                "length_preference": "短",
                "initiative": "接话",
                "context_focus": "当前消息",
                "avoid_topics": ["政治", "隐私"]
            }
        });

        let plan = parse_prompt_plan(&json).expect("解析失败");
        assert_eq!(plan.tone_profile.unwrap(), "轻松亲切");
        assert_eq!(plan.length_preference.unwrap(), "短");
        assert_eq!(plan.avoid_topics.len(), 2);
    }

    #[test]
    fn test_parse_prompt_plan_minimal() {
        let json = serde_json::json!({
            "reply_reference": "简单回复",
            "prompt_plan": {}
        });

        let plan = parse_prompt_plan(&json).expect("解析失败");
        assert!(plan.tone_profile.is_none());
        assert!(plan.avoid_topics.is_empty());
    }

    #[test]
    fn test_build_system_prompt() {
        // 使用一个 mock client 来测试构建
        let prompt = "你是会话控制中枢，负责在群聊场景下处理本轮对话。";
        assert!(prompt.contains("群聊"));
    }
}
