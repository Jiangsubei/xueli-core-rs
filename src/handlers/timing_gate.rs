use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::core::log_labels::{LOG_PROMPT_DIGEST, LOG_RETRY};
use crate::prelude::XueliResult;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};
use crate::traits::prompt_template::PromptTemplateLoader;
use crate::traits::timing_gate::{TimingContext, TimingDecision, TimingGateStrategy};

/// 规划信号 — TimingGate LLM 返回的即时规划信息
#[derive(Debug, Clone, Default)]
pub struct PlanningSignals {
    /// 用户情绪标签
    pub user_emotion_label: Option<String>,
    /// 情绪决策
    pub mood_decision: Option<MoodDecision>,
    /// 对话窗口分析
    pub conversation_window: Option<ConversationWindow>,
}

/// 情绪决策信号
#[derive(Debug, Clone)]
pub struct MoodDecision {
    /// 参与倾向: active/normal/reserved
    pub participation_bias: String,
    /// 回复能量: low/normal/high
    pub reply_energy: String,
    /// 风险姿态: safe/normal/careful
    pub risk_posture: String,
    /// 风格倾向: soft/playful/direct/calm/normal
    pub style_bias: String,
    /// 说明
    pub reason: String,
    /// 置信度
    pub confidence: f64,
}

/// 对话窗口分析
#[derive(Debug, Clone)]
pub struct ConversationWindow {
    /// 当前话题
    pub current_thread: String,
    /// 对话对象: assistant/group/specific_user/unknown
    pub speaker_target: String,
    /// 插话风险: low/medium/high
    pub interruption_risk: String,
    /// 对话开放度: open/semi_open/closed
    pub conversation_openness: String,
    /// 是否需要等待更多消息
    pub should_wait_for_more: bool,
    /// 说明
    pub reason: String,
    /// 置信度
    pub confidence: f64,
}

/// TimingGate 决策缓存条目
#[derive(Debug, Clone)]
struct CachedDecision {
    action: TimingDecision,
    reason: String,
    #[allow(dead_code)]
    planning_signals: PlanningSignals,
    cached_at: Instant,
    ttl: Duration,
}

impl CachedDecision {
    fn is_valid(&self) -> bool {
        self.cached_at.elapsed() < self.ttl
    }
}

/// 基于 LLM 的 TimingGate 实现
///
/// 对应 Python 版 `xueli/src/handlers/timing_gate.py`
pub struct DefaultTimingGate<A: AIClient, L: PromptTemplateLoader> {
    ai_client: Arc<A>,
    template_loader: Arc<L>,
    assistant_name: String,
    assistant_alias: String,
    locale: String,
    /// 缓存（群聊 key → 决策）
    cache: RwLock<HashMap<String, CachedDecision>>,
    /// 缓存 TTL
    cache_ttl_secs: f64,
    /// 最大重试次数
    max_retries: usize,
}

impl<A: AIClient, L: PromptTemplateLoader> DefaultTimingGate<A, L> {
    pub fn new(
        ai_client: Arc<A>,
        template_loader: Arc<L>,
        assistant_name: impl Into<String>,
        assistant_alias: impl Into<String>,
        locale: impl Into<String>,
    ) -> Self {
        Self {
            ai_client,
            template_loader,
            assistant_name: assistant_name.into(),
            assistant_alias: assistant_alias.into(),
            locale: locale.into(),
            cache: RwLock::new(HashMap::new()),
            cache_ttl_secs: 12.0,
            max_retries: 3,
        }
    }

    /// 限制缓存大小
    fn evict_cache(&self) {
        let mut cache = self.cache.write();
        if cache.len() > 500 {
            cache.retain(|_, v| v.is_valid());
        }
    }

    /// 构建系统提示词
    async fn build_system_prompt(&self) -> XueliResult<String> {
        let aliases_part_str = if self.assistant_alias.is_empty() {
            String::new()
        } else {
            format!("，别名是\u{201c}{}\u{201d}", self.assistant_alias)
        };
        let aliases_ref_str = if self.assistant_alias.is_empty() {
            String::new()
        } else {
            format!("或\u{201c}{}\u{201d}", self.assistant_alias)
        };

        let identity = self
            .template_loader
            .get_template(&self.locale, "timing_gate_identity.prompt")
            .await
            .map(|t| {
                let mut vars = HashMap::new();
                vars.insert("name", self.assistant_name.as_str());
                vars.insert("aliases_part", aliases_part_str.as_str());
                vars.insert("aliases_ref", aliases_ref_str.as_str());
                self.template_loader.render(&t, &vars)
            })
            .unwrap_or_else(|_| {
                if !self.assistant_alias.is_empty() {
                    format!(
                        "你的名字是\u{201c}{}\u{201d}，别名是\u{201c}{}\u{201d}。当群聊中有人提到你的名字或别名时，就是在对你说话，你应该选 reply。",
                        self.assistant_name, self.assistant_alias
                    )
                } else {
                    format!(
                        "你的名字是\u{201c}{}\u{201d}。当群聊中有人提到你的名字时，就是在对你说话，你应该选 reply。",
                        self.assistant_name
                    )
                }
            });

        let system = self
            .template_loader
            .get_template(&self.locale, "timing_gate.prompt")
            .await
            .map(|t| {
                let mut vars = HashMap::new();
                vars.insert("assistant_identity", identity.as_str());
                self.template_loader.render(&t, &vars)
            })
            .unwrap_or_else(|_| {
                format!(
                    "你是 Timing Gate。判断是否应回复当前消息。\n\n{identity}\n\n\
                    决策规则：\n\
                    - 被提到名字 → reply\n\
                    - 轻松闲聊可接话 → reply\n\
                    - 严肃争执/私密对话 → ignore\n\
                    - 信息不足 → wait\n\n\
                    输出 JSON: {{\"action\": \"reply|wait|ignore\", \"reason\": \"...\", \"planning_signals\": {{}}}}"
                )
            });

        Ok(system)
    }

    /// 构建 user prompt
    fn build_user_prompt(&self, ctx: &TimingContext) -> String {
        let mut parts: Vec<String> = Vec::new();

        // 发送者信息
        let sender = ctx
            .event
            .message
            .as_ref()
            .map(|m| m.sender_name.as_str())
            .unwrap_or("未知");
        let content = ctx
            .event
            .message
            .as_ref()
            .map(|m| m.text.as_str())
            .unwrap_or("");

        parts.push(format!("发送者={sender}"));
        parts.push(format!(
            "内容={}",
            if content.is_empty() {
                "用户发送了空文本"
            } else {
                content
            }
        ));

        // 对话活跃度信号
        if ctx.is_mentioned {
            parts.push("（此消息 @ 了你）".to_string());
        }

        parts.push("请只输出包含 action、reason、planning_signals 的 JSON。".to_string());

        parts.join("\n")
    }

    /// 解析 LLM 响应
    fn parse_response(
        &self,
        content: &str,
    ) -> XueliResult<(TimingDecision, String, PlanningSignals)> {
        let text = content.trim();
        if text.is_empty() {
            return Err("空响应".to_string().into());
        }

        // 尝试提取 JSON
        let json_str = if let Some(start) = text.find('{') {
            if let Some(end) = text.rfind('}') {
                &text[start..=end]
            } else {
                text
            }
        } else {
            text
        };

        let parsed: serde_json::Value =
            serde_json::from_str(json_str).map_err(|e| format!("JSON 解析失败: {e}"))?;

        let action_str = parsed
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("ignore")
            .to_lowercase();

        let reason = parsed
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let action = match action_str.as_str() {
            "reply" => TimingDecision::Reply,
            "wait" => TimingDecision::Wait(15.0),
            _ => TimingDecision::Ignore,
        };

        // 解析 planning_signals
        let planning_signals = parsed
            .get("planning_signals")
            .and_then(|v| v.as_object())
            .map(|signals| {
                let user_emotion_label = signals
                    .get("user_emotion_label")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let mood_decision = signals
                    .get("mood_decision")
                    .and_then(|v| v.as_object())
                    .map(|md| MoodDecision {
                        participation_bias: md
                            .get("participation_bias")
                            .and_then(|v| v.as_str())
                            .unwrap_or("normal")
                            .to_string(),
                        reply_energy: md
                            .get("reply_energy")
                            .and_then(|v| v.as_str())
                            .unwrap_or("normal")
                            .to_string(),
                        risk_posture: md
                            .get("risk_posture")
                            .and_then(|v| v.as_str())
                            .unwrap_or("normal")
                            .to_string(),
                        style_bias: md
                            .get("style_bias")
                            .and_then(|v| v.as_str())
                            .unwrap_or("normal")
                            .to_string(),
                        reason: md
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        confidence: md.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.5),
                    });

                let conversation_window = signals
                    .get("conversation_window")
                    .and_then(|v| v.as_object())
                    .map(|cw| ConversationWindow {
                        current_thread: cw
                            .get("current_thread")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        speaker_target: cw
                            .get("speaker_target")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        interruption_risk: cw
                            .get("interruption_risk")
                            .and_then(|v| v.as_str())
                            .unwrap_or("medium")
                            .to_string(),
                        conversation_openness: cw
                            .get("conversation_openness")
                            .and_then(|v| v.as_str())
                            .unwrap_or("semi_open")
                            .to_string(),
                        should_wait_for_more: cw
                            .get("should_wait_for_more")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        reason: cw
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        confidence: cw.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.5),
                    });

                PlanningSignals {
                    user_emotion_label,
                    mood_decision,
                    conversation_window,
                }
            })
            .unwrap_or_default();

        Ok((action, reason, planning_signals))
    }

    /// 构建缓存键
    fn build_cache_key(&self, ctx: &TimingContext) -> String {
        use sha2::{Digest, Sha256};
        let msg = ctx
            .event
            .message
            .as_ref()
            .map(|m| (m.sender_id.as_str(), m.text.as_str()))
            .unwrap_or_default();
        let payload = format!(
            "tg:v1:{}:{}:{}",
            msg.0,
            msg.1,
            if ctx.is_mentioned { "1" } else { "0" }
        );
        let digest = Sha256::digest(payload.as_bytes());
        format!("tg:{:x}", digest)
    }
}

#[async_trait::async_trait]
impl<A: AIClient, L: PromptTemplateLoader> TimingGateStrategy for DefaultTimingGate<A, L> {
    async fn should_reply(&self, ctx: &TimingContext) -> XueliResult<TimingDecision> {
        // 1. 检查缓存
        let cache_key = self.build_cache_key(ctx);
        {
            let cache = self.cache.read();
            if let Some(entry) = cache.get(&cache_key) {
                if entry.is_valid() {
                    tracing::debug!(
                        "[{LOG_PROMPT_DIGEST}] TimingGate 缓存命中: action={:?} reason={}",
                        entry.action,
                        entry.reason
                    );
                    return Ok(entry.action.clone());
                }
            }
        }

        // 2. 快速规则：被 @ 时大概率回复（即使 LLM 不可用也能 work）
        if ctx.is_mentioned {
            // 仍然通过 LLM 判定，但设置高回复倾向
        }

        // 3. LLM 调用
        let system_prompt = self.build_system_prompt().await?;
        let user_prompt = self.build_user_prompt(ctx);

        let messages = vec![
            ChatMessage::text("system", &system_prompt),
            ChatMessage::text("user", &user_prompt),
        ];

        let mut last_err = String::new();
        for attempt in 0..self.max_retries {
            let request = ChatCompletionRequest {
                model: "gpt-4o-mini".to_string(),
                messages: messages.clone(),
                temperature: Some(0.1),
                max_tokens: Some(512),
                stream: false,
                tools: None,
                tool_choice: None,
                extra_params: HashMap::new(),
            };

            match self.ai_client.chat_completion(&request).await {
                Ok(response) => {
                    tracing::debug!(
                        "[{LOG_PROMPT_DIGEST}] TimingGate 响应: {}",
                        &response.content.chars().take(200).collect::<String>()
                    );

                    match self.parse_response(&response.content) {
                        Ok((decision, reason, signals)) => {
                            tracing::info!(
                                "TimingGate decision={:?} reason={} emotion={:?}",
                                decision,
                                reason,
                                signals.user_emotion_label
                            );

                            // 写入缓存
                            self.evict_cache();
                            let mut cache = self.cache.write();
                            cache.insert(
                                cache_key,
                                CachedDecision {
                                    action: decision.clone(),
                                    reason,
                                    planning_signals: signals,
                                    cached_at: Instant::now(),
                                    ttl: Duration::from_secs_f64(self.cache_ttl_secs),
                                },
                            );

                            return Ok(decision);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[{LOG_RETRY}] TimingGate 响应解析失败 (attempt {attempt}): {e}"
                            );
                            last_err = e.to_string();
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("[{LOG_RETRY}] TimingGate AI 调用失败 (attempt {attempt}): {e}");
                    last_err = e.to_string();
                }
            }
        }

        // 4. 降级规则
        tracing::warn!("TimingGate LLM 全部失败，退回规则路径: {}", last_err);

        if ctx.is_mentioned {
            Ok(TimingDecision::Reply)
        } else if ctx.time_since_last_reply_secs > 30.0
            && ctx.message_count_in_window > 0
            && ctx.conversation_active
        {
            Ok(TimingDecision::Wait(
                (rand::random::<f64>() * 30.0 + 5.0).min(60.0),
            ))
        } else {
            Ok(TimingDecision::Ignore)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::platform_types::{EventType, InboundEvent};
    use crate::core::scope::ChatScope;
    use crate::core::types::UserMessage;
    use chrono::Utc;

    fn make_context(mentioned: bool, text: &str) -> TimingContext {
        TimingContext {
            event: InboundEvent {
                id: "evt1".to_string(),
                platform: "test".to_string(),
                event_type: EventType::Message,
                message: Some(UserMessage {
                    id: "msg1".to_string(),
                    sender_id: "u1".to_string(),
                    sender_name: "测试用户".to_string(),
                    text: text.to_string(),
                    timestamp: Utc::now(),
                    scope: ChatScope::Private,
                    is_mention: mentioned,
                }),
                raw_payload: None,
                received_at: Utc::now(),
                session: None,
            },
            is_mentioned: mentioned,
            conversation_active: true,
            time_since_last_reply_secs: 10.0,
            message_count_in_window: 3,
        }
    }

    #[test]
    fn test_parse_response_reply() {
        let gate = DefaultTimingGate::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "测试",
            "",
            "zh-CN",
        );
        let json = r#"{"action": "reply", "reason": "消息提到助手名", "planning_signals": {"user_emotion_label": "开心"}}"#;
        let (decision, reason, signals) = gate.parse_response(json).unwrap();
        assert_eq!(decision, TimingDecision::Reply);
        assert!(reason.contains("助手"));
        assert_eq!(signals.user_emotion_label.unwrap(), "开心");
    }

    #[test]
    fn test_parse_response_ignore() {
        let gate = DefaultTimingGate::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "测试",
            "",
            "zh-CN",
        );
        let json = r#"{"action": "ignore", "reason": "两人私密对话", "planning_signals": {}}"#;
        let (decision, reason, _) = gate.parse_response(json).unwrap();
        assert_eq!(decision, TimingDecision::Ignore);
        assert!(reason.contains("私密"));
    }

    #[test]
    fn test_parse_response_wait() {
        let gate = DefaultTimingGate::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "测试",
            "",
            "zh-CN",
        );
        let json = r#"{"action": "wait", "reason": "信息不足", "planning_signals": {}}"#;
        let (decision, _, _) = gate.parse_response(json).unwrap();
        assert_eq!(decision, TimingDecision::Wait(15.0));
    }

    #[test]
    fn test_parse_response_with_planning_signals() {
        let gate = DefaultTimingGate::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "测试",
            "",
            "zh-CN",
        );
        let json = r#"{
            "action": "reply",
            "reason": "轻松闲聊",
            "planning_signals": {
                "user_emotion_label": "开心",
                "mood_decision": {
                    "participation_bias": "active",
                    "reply_energy": "high",
                    "risk_posture": "safe",
                    "style_bias": "playful",
                    "reason": "气氛轻松",
                    "confidence": 0.85
                },
                "conversation_window": {
                    "current_thread": "日常闲聊",
                    "speaker_target": "group",
                    "interruption_risk": "low",
                    "conversation_openness": "open",
                    "should_wait_for_more": false,
                    "reason": "群聊开放对话",
                    "confidence": 0.9
                }
            }
        }"#;
        let (decision, _, signals) = gate.parse_response(json).unwrap();
        assert_eq!(decision, TimingDecision::Reply);
        assert!(signals.mood_decision.is_some());
        assert_eq!(
            signals.mood_decision.as_ref().unwrap().participation_bias,
            "active"
        );
        assert!(signals.conversation_window.is_some());
        assert!(
            !signals
                .conversation_window
                .as_ref()
                .unwrap()
                .should_wait_for_more
        );
    }

    #[test]
    fn test_build_cache_key_different_on_mention() {
        let gate = DefaultTimingGate::new(
            Arc::new(crate::services::ai_client::NoopAIClient),
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "测试",
            "",
            "zh-CN",
        );
        let ctx1 = make_context(false, "你好");
        let ctx2 = make_context(true, "你好");
        let key1 = gate.build_cache_key(&ctx1);
        let key2 = gate.build_cache_key(&ctx2);
        assert_ne!(key1, key2);
    }
}
