use std::collections::HashMap;
use std::sync::Arc;

use crate::core::platform_types::InboundEvent;
use crate::core::types::{MessageHandlingPlan, ReplyPlan};
use crate::handlers::context_builder::ConversationContext;

pub use crate::core::types::{MemoryProfile, PromptPlan, PromptSectionPolicy, SectionIntensity};
use crate::handlers::shared::prompt_planner::PromptPlanner;
use crate::prelude::XueliResult;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};
use crate::traits::prompt_template::PromptTemplateLoader;

/// 会话规划器 — 调用 LLM 规划回复策略
///
/// 对应 Python 版 `xueli/src/handlers/conversation/planner.py`
pub struct ConversationPlanner<A: AIClient, L: PromptTemplateLoader> {
    ai_client: Arc<A>,
    template_loader: Arc<L>,
    model: String,
    assistant_name: String,
    assistant_alias: String,
    locale: String,
    emoji_enabled: bool,
    prompt_planner: PromptPlanner,
}

impl<A: AIClient, L: PromptTemplateLoader> ConversationPlanner<A, L> {
    pub fn new(
        ai_client: Arc<A>,
        template_loader: Arc<L>,
        model: &str,
        assistant_name: impl Into<String>,
        assistant_alias: impl Into<String>,
        locale: impl Into<String>,
    ) -> Self {
        Self {
            ai_client,
            template_loader,
            model: model.to_string(),
            assistant_name: assistant_name.into(),
            assistant_alias: assistant_alias.into(),
            locale: locale.into(),
            emoji_enabled: false,
            prompt_planner: PromptPlanner::default(),
        }
    }

    /// 启用表情包决策段
    pub fn with_emoji_enabled(mut self, enabled: bool) -> Self {
        self.emoji_enabled = enabled;
        self
    }

    /// 规划回复 — 调用 LLM 生成完整 MessageHandlingPlan
    pub async fn plan(
        &self,
        event: &InboundEvent,
        context: &ConversationContext,
    ) -> XueliResult<MessageHandlingPlan> {
        let is_group = context.is_group;

        let system_prompt = self.build_system_prompt(is_group).await?;
        let user_prompt = self.build_user_prompt(event, context);

        let messages = vec![
            ChatMessage::text("system", system_prompt),
            ChatMessage::text("user", user_prompt),
        ];

        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(0.3),
            max_tokens: Some(2048),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = self.ai_client.chat_completion(&request).await?;
        let content = response.content.clone();

        let decision_json = match Self::extract_json(&content) {
            Some(json) => json,
            None => {
                tracing::warn!("[Planner] LLM 输出不是有效 JSON，使用 fallback");
                return Ok(self.build_fallback_plan(event, &content, context));
            }
        };

        Ok(self.parse_plan(event, context, &decision_json))
    }

    /// 构建 system prompt
    async fn build_system_prompt(&self, is_group: bool) -> XueliResult<String> {
        let chat_mode_label = if is_group { "群聊" } else { "私聊" };

        let aliases_part = if self.assistant_alias.is_empty() {
            String::new()
        } else {
            format!("，别名是\u{201c}{}\u{201d}", self.assistant_alias)
        };
        let aliases_ref = if self.assistant_alias.is_empty() {
            String::new()
        } else {
            format!("或\u{201c}{}\u{201d}", self.assistant_alias)
        };

        let assistant_identity = match self
            .template_loader
            .get_template(&self.locale, "identity.prompt")
            .await
        {
            Ok(tpl) => {
                let mut vars = HashMap::new();
                vars.insert("name", self.assistant_name.as_str());
                vars.insert("aliases_part", aliases_part.as_str());
                vars.insert("aliases_ref", aliases_ref.as_str());
                let rendered = self.template_loader.render(&tpl, &vars);
                if rendered.is_empty() {
                    Self::fallback_identity(&self.assistant_name, &self.assistant_alias, is_group)
                } else {
                    rendered
                }
            }
            Err(_) => {
                Self::fallback_identity(&self.assistant_name, &self.assistant_alias, is_group)
            }
        };

        let emoji_section = if self.emoji_enabled {
            match self
                .template_loader
                .get_template(&self.locale, "planner_emoji_section.prompt")
                .await
            {
                Ok(tpl) => self.template_loader.render(&tpl, &HashMap::new()),
                Err(_) => String::new(),
            }
        } else {
            String::new()
        };

        let decision_output_schema = self
            .prompt_planner
            .decision_output_schema(self.emoji_enabled);

        match self
            .template_loader
            .get_template(&self.locale, "planner.prompt")
            .await
        {
            Ok(tpl) => {
                let mut vars = HashMap::new();
                vars.insert("chat_mode_label", chat_mode_label);
                vars.insert("assistant_identity", assistant_identity.as_str());
                vars.insert("emoji_section", emoji_section.as_str());
                vars.insert("decision_output_schema", decision_output_schema.as_str());
                Ok(self.template_loader.render(&tpl, &vars))
            }
            Err(e) => {
                tracing::warn!("[Planner] 加载 planner.prompt 失败: {e}，使用硬编码 fallback");
                Ok(Self::fallback_system_prompt(
                    chat_mode_label,
                    &assistant_identity,
                    &emoji_section,
                    &decision_output_schema,
                ))
            }
        }
    }

    /// 构建 user prompt
    fn build_user_prompt(&self, _event: &InboundEvent, context: &ConversationContext) -> String {
        let mut sections: Vec<String> = Vec::new();

        // 时间上下文
        if let Some(ref hint) = context.continuity_hint {
            sections.push(format!("【时间连续性】\n{}", hint));
        }

        // 近期对话历史
        if !context.recent_messages.is_empty() {
            sections.push(format!(
                "【近期对话历史】\n{}",
                context.recent_messages.join("\n")
            ));
        }

        // 记忆上下文
        if let Some(ref facts) = context.person_facts {
            if !facts.is_empty() {
                sections.push(format!("【人物事实】\n{}", facts.join("\n")));
            }
        }
        if let Some(ref mem) = context.persistent_memory_context {
            if !mem.is_empty() {
                sections.push(format!("【持久记忆】\n{}", mem));
            }
        }
        if let Some(ref mem) = context.dynamic_memory {
            if !mem.is_empty() {
                sections.push(format!("【动态记忆】\n{}", mem));
            }
        }
        if let Some(ref recall) = context.precise_recall {
            if !recall.is_empty() {
                sections.push(format!("【精准回忆】\n{}", recall));
            }
        }

        // 角色卡 / 叙事
        if let Some(ref snapshot) = context.character_card_snapshot {
            if !snapshot.relationship_state_summary.is_empty() {
                sections.push(format!(
                    "【关系摘要】\n{}",
                    snapshot.relationship_state_summary
                ));
            }
            if !snapshot.relationship_tone_hint.is_empty() {
                sections.push(format!("【关系语气】\n{}", snapshot.relationship_tone_hint));
            }
        }
        if let Some(ref summary) = context.narrative_thread_summary {
            if !summary.is_empty() {
                sections.push(format!("【叙事线摘要】\n{}", summary));
            }
        }

        // 视觉上下文
        if let Some(ref vision) = context.vision_description {
            if !vision.is_empty() {
                sections.push(format!("【视觉上下文】\n{}", vision));
            }
        }

        // 用户情绪
        if let Some(ref label) = context.user_emotion_label {
            if !label.is_empty() {
                sections.push(format!("【用户情绪】\n{}", label));
            }
        }

        // 谨慎信号
        if let Some(ref caution) = context.caution_signal {
            if !caution.is_empty() {
                sections.push(format!("【谨慎信号】\n{}", Self::format_caution(caution)));
            }
        }

        // 当前消息
        let current_text = if context.user_message.trim().is_empty() {
            "用户发送了空文本".to_string()
        } else {
            context.user_message.clone()
        };
        sections.push(format!("【当前消息】\n{}", current_text));

        sections.join("\n\n")
    }

    /// 解析 LLM 返回的决策 JSON 为 MessageHandlingPlan
    fn parse_plan(
        &self,
        event: &InboundEvent,
        context: &ConversationContext,
        decision: &serde_json::Value,
    ) -> MessageHandlingPlan {
        let decision_obj = decision.as_object();

        let prompt_plan = self
            .prompt_planner
            .parse_prompt_plan(decision_obj, event, context.is_group, context.is_first_turn)
            .unwrap_or_default();

        let reply_reference = decision
            .get("reply_reference")
            .or_else(|| decision.get("reply_guidance"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let reason = decision
            .get("reason")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "Planner 完成规划".to_string());

        let predicted_user_response = decision
            .get("predicted_user_response")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let expected_effect = decision
            .get("expected_effect")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let narrative_signal = decision
            .get("narrative_signal")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let reply_adaptation_signal = decision
            .get("reply_adaptation_signal")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let mood_state = decision
            .get("mood_state")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mood_adjustments = decision
            .get("mood_adjustments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let system_recommendations = decision
            .get("system_recommendations")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let caution_hint = decision
            .get("caution_hint")
            .and_then(|v| v.as_object())
            .cloned();

        let mut reply_context: HashMap<String, serde_json::Value> = HashMap::new();
        if !predicted_user_response.is_empty() {
            reply_context.insert(
                "predicted_user_response".to_string(),
                serde_json::json!(predicted_user_response),
            );
        }
        if !expected_effect.is_empty() {
            reply_context.insert(
                "expected_effect".to_string(),
                serde_json::json!(expected_effect),
            );
        }
        if let Some(ref state) = mood_state {
            reply_context.insert("mood_state".to_string(), serde_json::json!(state));
        }
        if let Some(obj) = caution_hint.clone() {
            reply_context.insert("caution_hint".to_string(), serde_json::Value::Object(obj));
        }
        reply_context.insert("narrative_signal".to_string(), narrative_signal.clone());
        reply_context.insert(
            "reply_adaptation_signal".to_string(),
            reply_adaptation_signal.clone(),
        );
        reply_context.insert(
            "system_recommendations".to_string(),
            system_recommendations.clone(),
        );

        // 将视觉分析结果注入 reply_context，供 reply pipeline 复用
        if let Some(ref vision) = context.vision_description {
            let desc = vision
                .strip_prefix("[图片] ")
                .unwrap_or(vision)
                .trim()
                .to_string();
            let vision_available = !desc.is_empty()
                && !desc.contains("分析失败")
                && !desc.contains("待处理")
                && !desc.contains("未返回可用描述");
            reply_context.insert(
                "vision_analysis".to_string(),
                serde_json::json!({
                    "merged_description": desc,
                    "vision_available": vision_available,
                }),
            );
        }

        let mut planning_signals: HashMap<String, serde_json::Value> = HashMap::new();
        planning_signals.insert("narrative_signal".to_string(), narrative_signal);
        planning_signals.insert(
            "reply_adaptation_signal".to_string(),
            reply_adaptation_signal,
        );
        planning_signals.insert("mood_adjustments".to_string(), mood_adjustments);
        planning_signals.insert("system_recommendations".to_string(), system_recommendations);

        let planner_caution_hint = caution_hint
            .as_ref()
            .and_then(|c| c.get("reply_guidance"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let risk_posture = caution_hint
            .as_ref()
            .and_then(|c| c.get("risk_posture"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let _reply_plan = ReplyPlan {
            id: uuid::Uuid::new_v4().to_string(),
            target_message_id: event
                .message
                .as_ref()
                .map(|m| m.id.clone())
                .unwrap_or_default(),
            topic: decision
                .get("narrative_signal")
                .and_then(|v| v.get("narrative_summary"))
                .and_then(|v| v.as_str())
                .map(String::from),
            style: prompt_plan.tone_profile.clone().into(),
            memory_recall_needed: false,
            use_emoji: decision
                .get("emoji_should_send")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            priority: 0,
        };

        MessageHandlingPlan {
            action: "reply".to_string(),
            reason,
            source: "planner".to_string(),
            should_reply: true,
            raw_decision: Some(decision.clone()),
            reply_context,
            prompt_plan: Some(prompt_plan),
            reply_reference,
            planning_signals,
            planner_caution_hint,
            risk_posture,
            cached_context: None,
        }
    }

    /// 构建 fallback plan（LLM 输出无法解析时）
    fn build_fallback_plan(
        &self,
        _event: &InboundEvent,
        raw_content: &str,
        context: &ConversationContext,
    ) -> MessageHandlingPlan {
        let mut reply_context = HashMap::new();
        reply_context.insert(
            "raw_content".to_string(),
            serde_json::json!(raw_content.to_string()),
        );

        let default_prompt_plan = self.prompt_planner.default_prompt_plan(
            context.is_group,
            context.continuity_hint.as_deref().unwrap_or("unknown"),
            context.is_first_turn,
            context.follows_assistant_recently,
        );

        MessageHandlingPlan {
            action: "reply".to_string(),
            reason: "Planner LLM 输出解析失败，使用默认规划".to_string(),
            source: "planner_fallback".to_string(),
            should_reply: true,
            raw_decision: None,
            reply_context,
            prompt_plan: Some(default_prompt_plan),
            reply_reference: raw_content.to_string(),
            planning_signals: HashMap::new(),
            planner_caution_hint: None,
            risk_posture: None,
            cached_context: None,
        }
    }

    /// 从文本中提取 JSON 对象
    fn extract_json(text: &str) -> Option<serde_json::Value> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        // 先尝试完整解析
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            if v.is_object() {
                return Some(v);
            }
        }
        // 尝试截取第一个 {} 块
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        if end <= start {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(&text[start..=end]).ok()
    }

    fn fallback_identity(name: &str, alias: &str, _is_group: bool) -> String {
        if alias.is_empty() {
            format!("你的名字是\u{201c}{}\u{201d}", name)
        } else {
            format!(
                "你的名字是\u{201c}{}\u{201d}，别名是\u{201c}{}\u{201d}",
                name, alias
            )
        }
    }

    fn fallback_system_prompt(
        chat_mode: &str,
        identity: &str,
        emoji_section: &str,
        schema: &str,
    ) -> String {
        format!(
            "你是会话控制中枢，负责在{chat_mode}场景下处理本轮对话。\n\n\
            {identity}\n\n\
            上游已决定回复，你输出本轮回复所需的全部规划与决策。\n\n\
            {emoji_section}\n\n\
            【输出格式】\n\
            你必须只输出 JSON，格式如下：\n\
            {schema}\n\
            不要输出 JSON 以外的任何内容。"
        )
    }

    fn format_caution(caution: &HashMap<String, String>) -> String {
        let mut lines = Vec::new();
        if let Some(level) = caution.get("caution_level") {
            lines.push(format!("谨慎级别: {}", level));
        }
        if let Some(reasons) = caution.get("caution_reasons") {
            lines.push(format!("原因: {}", reasons));
        }
        if let Some(guidance) = caution.get("reply_guidance") {
            lines.push(format!("回复建议: {}", guidance));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::platform_types::{EventType, InboundEvent};
    use crate::core::scope::ChatScope;
    use crate::services::ai_client::NoopAIClient;
    use crate::services::prompt_loader::NoopPromptTemplateLoader;

    fn make_event() -> InboundEvent {
        InboundEvent {
            id: "e1".into(),
            platform: "test".into(),
            event_type: EventType::Message,
            message: Some(crate::core::types::UserMessage {
                id: "m1".into(),
                sender_id: "u1".into(),
                sender_name: "T".into(),
                text: "hi".into(),
                timestamp: chrono::Utc::now(),
                scope: ChatScope::Private,
                is_mention: false,
            }),
            raw_payload: None,
            received_at: chrono::Utc::now(),
            session: None,
            ..Default::default()
        }
    }

    fn make_context(user_message: &str) -> ConversationContext {
        ConversationContext {
            user_message: user_message.to_string(),
            recent_messages: Vec::new(),
            conversation_key: "test".to_string(),
            user_id: "u1".to_string(),
            scope: ChatScope::Private,
            is_group: false,
            is_first_turn: true,
            person_facts: None,
            persistent_memory_context: None,
            dynamic_memory: None,
            session_restore: None,
            precise_recall: None,
            vision_description: None,
            continuity_hint: None,
            follows_assistant_recently: false,
            recent_message_count: 0,
            character_card_snapshot: None,
            narrative_thread_summary: None,
            narrative_thread_label: None,
            narrative_self: None,
            caution_signal: None,
            planning_signals: None,
            user_emotion_label: None,
            style_guide: None,
            soft_uncertainty_signals: None,
            drive_context: None,
            caution_events: None,
            user_profile_signal: None,
            style_adaptation_signal: None,
            relationship_state_signal: None,
        }
    }

    #[test]
    fn test_extract_json_full() {
        let text = r#"{"reply_reference":"test","prompt_plan":{}}"#;
        let json =
            ConversationPlanner::<NoopAIClient, NoopPromptTemplateLoader>::extract_json(text);
        assert!(json.is_some());
        assert_eq!(json.unwrap()["reply_reference"].as_str().unwrap(), "test");
    }

    #[test]
    fn test_extract_json_embedded() {
        let text = "这里有一些解释文字\n{\"reply_reference\":\"test\"}\n更多文字";
        let json =
            ConversationPlanner::<NoopAIClient, NoopPromptTemplateLoader>::extract_json(text);
        assert!(json.is_some());
    }

    #[test]
    fn test_build_fallback_plan() {
        let planner = ConversationPlanner::new(
            Arc::new(NoopAIClient),
            Arc::new(NoopPromptTemplateLoader),
            "gpt-4o-mini",
            "测试助手",
            "",
            "zh-CN",
        );
        let event = make_event();
        let ctx = make_context("你好");
        let plan = planner.build_fallback_plan(&event, "raw", &ctx);
        assert!(plan.should_reply);
        assert_eq!(plan.source, "planner_fallback");
    }
}
