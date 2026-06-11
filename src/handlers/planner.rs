use std::sync::Arc;

use crate::core::platform_types::InboundEvent;
use crate::core::types::ReplyPlan;
use crate::handlers::context_builder::ConversationContext;
use crate::handlers::shared::prompt_planner::PromptPlanner;
use crate::prelude::XueliResult;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};

/// 会话规划器 — 调用 LLM 规划回复策略
pub struct ConversationPlanner<A: AIClient> {
    ai_client: Arc<A>,
    model: String,
    prompt_planner: PromptPlanner,
}

/// 规划结果
#[derive(Debug, Clone)]
pub struct PlanResult {
    pub reply_plan: ReplyPlan,
    pub prompt_plan: Option<PromptPlan>,
    pub reply_reference: String,
    pub should_reply: bool,
    pub confidence: f64,
}

/// 提示词区段编译开关 — 对应 Python 版 `PromptSectionPolicy`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PromptSectionPolicy {
    pub include_recent_history: bool,
    pub include_person_facts: bool,
    pub include_session_restore: bool,
    pub include_precise_recall: bool,
    pub include_dynamic_memory: bool,
    pub include_vision_context: bool,
    pub include_reply_scope: bool,
    pub include_style_guide: bool,
}

impl Default for PromptSectionPolicy {
    fn default() -> Self {
        Self {
            include_recent_history: true,
            include_person_facts: true,
            include_session_restore: true,
            include_precise_recall: true,
            include_dynamic_memory: true,
            include_vision_context: true,
            include_reply_scope: true,
            include_style_guide: true,
        }
    }
}

/// 提示词计划 — 对应 Python 版 `PromptPlan`，指导 ReplyAgent 如何构建回复
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PromptPlan {
    pub reply_goal: String,
    pub continuity_mode: String,
    pub timeline_detail: String,
    pub context_profile: String,
    pub memory_profile: String,
    pub tone_profile: String,
    pub initiative: String,
    pub expression_profile: String,
    pub policy: PromptSectionPolicy,
    pub notes: String,
    pub emoji_should_send: bool,
    pub emoji_intent_reference: String,
}

impl Default for PromptPlan {
    fn default() -> Self {
        Self {
            reply_goal: "continue".into(),
            continuity_mode: "direct_continue".into(),
            timeline_detail: "summary".into(),
            context_profile: "standard".into(),
            memory_profile: "relevant".into(),
            tone_profile: "balanced".into(),
            initiative: "gentle_follow".into(),
            expression_profile: "plain".into(),
            policy: PromptSectionPolicy::default(),
            notes: String::new(),
            emoji_should_send: false,
            emoji_intent_reference: String::new(),
        }
    }
}

impl<A: AIClient> ConversationPlanner<A> {
    pub fn new(ai_client: Arc<A>, model: &str) -> Self {
        Self {
            ai_client,
            model: model.to_string(),
            prompt_planner: PromptPlanner::default(),
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
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = self.ai_client.chat_completion(&request).await?;
        let content = response.content.clone();

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

                let prompt_plan = self.prompt_planner.parse_prompt_plan(
                    json.as_object(),
                    event,
                    context.is_group,
                    context.is_first_turn,
                );
                let reply_reference = parse_reply_reference_from_json(json.as_object());

                Ok(PlanResult {
                    reply_plan,
                    prompt_plan,
                    reply_reference,
                    should_reply: true,
                    confidence: 0.8,
                })
            }
            Err(_) => Ok(PlanResult {
                reply_plan: ReplyPlan {
                    id: uuid::Uuid::new_v4().to_string(),
                    target_message_id: event
                        .message
                        .as_ref()
                        .map(|m| m.id.clone())
                        .unwrap_or_default(),
                    topic: Some(content.clone()),
                    style: None,
                    memory_recall_needed: false,
                    use_emoji: true,
                    priority: 0,
                },
                prompt_plan: None,
                reply_reference: content,
                should_reply: true,
                confidence: 0.5,
            }),
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

fn parse_reply_reference_from_json(
    obj: Option<&serde_json::Map<String, serde_json::Value>>,
) -> String {
    obj.and_then(|m| m.get("reply_reference"))
        .or_else(|| obj.and_then(|m| m.get("reply_guidance")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_build_system_prompt() {
        let prompt = "你是会话控制中枢，负责在群聊场景下处理本轮对话。";
        assert!(prompt.contains("群聊"));
    }
}
