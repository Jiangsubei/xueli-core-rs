use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::core::config::EmojiConfig;
use crate::core::platform_types::{InboundEvent, ReplyAction, SessionRef};
use crate::emoji::database::EmojiDatabase;
use crate::prelude::{AIClient, PromptTemplateLoader, XueliResult};
use crate::traits::ai_client::{ChatCompletionRequest, ChatMessage};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmojiReplyDecision {
    pub should_send: bool,
    pub emoji_file_ids: Vec<String>,
    pub reply_intent: Option<String>,
    pub confidence: f64,
}

impl Default for EmojiReplyDecision {
    fn default() -> Self {
        Self {
            should_send: false,
            emoji_file_ids: Vec::new(),
            reply_intent: None,
            confidence: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmojiReplySelection {
    pub file_ids: Vec<String>,
    pub intent: Option<String>,
    pub confidence: f64,
    pub skip_reason: String,
}

pub struct EmojiReplyService<L: PromptTemplateLoader> {
    config: EmojiConfig,
    ai_client: Arc<dyn AIClient>,
    emoji_database: Arc<EmojiDatabase>,
    model_name: String,
    sent_emoji_cache: Mutex<HashMap<String, Vec<String>>>,
    cooldown_state: Mutex<HashMap<String, Instant>>,
    template_loader: Arc<L>,
    default_emotion_labels: Vec<String>,
    default_reply_tones: Vec<String>,
}

impl<L: PromptTemplateLoader> EmojiReplyService<L> {
    pub fn new(
        config: EmojiConfig,
        ai_client: Arc<dyn AIClient>,
        emoji_database: Arc<EmojiDatabase>,
        template_loader: Arc<L>,
        model_name: String,
    ) -> Self {
        let default_emotion_labels = vec![
            "开心".to_string(),
            "难过".to_string(),
            "生气".to_string(),
            "惊讶".to_string(),
            "喜欢".to_string(),
            "再见".to_string(),
            "鼓励".to_string(),
        ];
        let default_reply_tones = vec![
            "温暖".to_string(),
            "俏皮".to_string(),
            "安慰".to_string(),
            "共鸣".to_string(),
            "庆祝".to_string(),
            "告别".to_string(),
            "敷衍".to_string(),
        ];
        Self {
            config,
            ai_client,
            emoji_database,
            model_name,
            sent_emoji_cache: Mutex::new(HashMap::new()),
            cooldown_state: Mutex::new(HashMap::new()),
            template_loader,
            default_emotion_labels,
            default_reply_tones,
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled && self.config.reply_enabled
    }

    pub async fn plan_follow_up(
        &self,
        event: &InboundEvent,
        user_message: &str,
        assistant_reply: &str,
        reply_context: &HashMap<String, serde_json::Value>,
        trace_id: &str,
        intent_reference: &str,
        window_messages: Option<&Vec<serde_json::Value>>,
    ) -> XueliResult<Option<EmojiReplySelection>> {
        if !self.enabled() {
            return Ok(Some(EmojiReplySelection {
                file_ids: Vec::new(),
                intent: None,
                confidence: 0.0,
                skip_reason: "feature_disabled".into(),
            }));
        }

        let session = event.get_session();
        if !session.scope.is_group() {
            return Ok(Some(EmojiReplySelection {
                file_ids: Vec::new(),
                intent: None,
                confidence: 0.0,
                skip_reason: "unsupported_message_type".into(),
            }));
        }

        if assistant_reply.trim().is_empty() {
            return Ok(Some(EmojiReplySelection {
                file_ids: Vec::new(),
                intent: None,
                confidence: 0.0,
                skip_reason: "empty_reply".into(),
            }));
        }

        let group_id = session.scope.group_id().unwrap_or("").to_string();
        if self.group_cooldown_active(&group_id) {
            return Ok(Some(EmojiReplySelection {
                file_ids: Vec::new(),
                intent: None,
                confidence: 0.0,
                skip_reason: "group_cooldown".into(),
            }));
        }

        let decision = self
            .decide_reply_intent(
                user_message,
                assistant_reply,
                reply_context,
                trace_id,
                intent_reference,
                window_messages,
            )
            .await?;

        if !decision.should_send {
            return Ok(Some(EmojiReplySelection {
                file_ids: Vec::new(),
                intent: decision.reply_intent,
                confidence: decision.confidence,
                skip_reason: "model_declined".into(),
            }));
        }

        let style_adaptation = reply_context
            .get("style_adaptation")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let relationship_summary = reply_context
            .get("relationship_summary")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        for s in &[style_adaptation, relationship_summary] {
            if s.contains("less_emoji") || s.contains("less_sticker") || s.contains("少发表情")
            {
                return Ok(Some(EmojiReplySelection {
                    file_ids: Vec::new(),
                    intent: decision.reply_intent,
                    confidence: decision.confidence,
                    skip_reason: "user_prefers_less_emoji".into(),
                }));
            }
        }

        let intent = decision.reply_intent.as_deref().unwrap_or("");
        let candidates = self.emoji_database.find_by_intent(intent)?;
        let cooled: Vec<&crate::emoji::database::StickerRecord> = candidates
            .iter()
            .filter(|c| !self.emoji_cooldown_active(c))
            .collect();

        if cooled.is_empty() {
            return Ok(Some(EmojiReplySelection {
                file_ids: Vec::new(),
                intent: decision.reply_intent,
                confidence: decision.confidence,
                skip_reason: "no_candidate".into(),
            }));
        }

        let selected = self.weighted_pick(&cooled, &group_id);
        match selected {
            Some(file_hash) => Ok(Some(EmojiReplySelection {
                file_ids: vec![file_hash],
                intent: decision.reply_intent,
                confidence: decision.confidence,
                skip_reason: String::new(),
            })),
            None => Ok(Some(EmojiReplySelection {
                file_ids: Vec::new(),
                intent: decision.reply_intent,
                confidence: decision.confidence,
                skip_reason: "selection_failed".into(),
            })),
        }
    }

    pub fn build_follow_up_action(
        &self,
        selection: &EmojiReplySelection,
        session: &SessionRef,
    ) -> Option<ReplyAction> {
        let emoji_id = selection.file_ids.first()?;
        // 尝试读取贴纸文件获取路径
        let record = self.emoji_database.sync_get_record(emoji_id);
        if let Some(ref rec) = record {
            if !rec.file_path.is_empty() && std::path::Path::new(&rec.file_path).exists() {
                match std::fs::read(&rec.file_path) {
                    Ok(_file_bytes) => {
                        return Some(ReplyAction {
                            scope: session.scope.clone(),
                            text: String::new(),
                            reply_to: None,
                            image_url: None,
                            emoji_id: Some(format!("sticker:{}", emoji_id)),
                        });
                    }
                    Err(_) => {}
                }
            }
        }
        Some(ReplyAction {
            scope: session.scope.clone(),
            text: String::new(),
            reply_to: None,
            image_url: None,
            emoji_id: Some(format!("sticker:{}", emoji_id)),
        })
    }

    pub async fn mark_follow_up_sent(
        &self,
        event: &InboundEvent,
        selection: &EmojiReplySelection,
    ) -> XueliResult<()> {
        let session = event.get_session();
        let group_id = session.scope.group_id().unwrap_or("").to_string();

        {
            let mut cache = self.sent_emoji_cache.lock();
            let entry = cache.entry(group_id.clone()).or_default();
            for file_id in &selection.file_ids {
                entry.push(file_id.clone());
            }
            if entry.len() > 50 {
                entry.drain(..entry.len() - 50);
            }
        }

        {
            self.cooldown_state
                .lock()
                .insert(group_id.clone(), Instant::now());
        }

        for file_id in &selection.file_ids {
            let _ = self
                .emoji_database
                .mark_auto_reply_sent_async(file_id)
                .await;
        }

        Ok(())
    }

    async fn decide_reply_intent(
        &self,
        user_message: &str,
        assistant_reply: &str,
        reply_context: &HashMap<String, serde_json::Value>,
        _trace_id: &str,
        intent_reference: &str,
        window_messages: Option<&Vec<serde_json::Value>>,
    ) -> XueliResult<EmojiReplyDecision> {
        let window_messages_val = window_messages
            .cloned()
            .or_else(|| {
                reply_context
                    .get("window_messages")
                    .and_then(|v| v.as_array())
                    .map(|a| a.to_vec())
            })
            .unwrap_or_default();
        let recent_window: Vec<&serde_json::Value> =
            window_messages_val.iter().rev().take(4).rev().collect();

        let mut context_parts = Vec::new();
        for item in &recent_window {
            let uid = item.get("user_id").and_then(|v| v.as_str()).unwrap_or("");
            let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let is_latest = item
                .get("is_latest")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let latest_mark = if is_latest { "（当前消息）" } else { "" };
            if !uid.is_empty() && !text.is_empty() {
                context_parts.push(format!("{}: {}{}", uid, text, latest_mark));
            }
        }

        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("【用户消息】\n{}", user_message));
        parts.push(format!("【助手回复】\n{}", assistant_reply));

        if !intent_reference.is_empty() {
            parts.push(format!("【表情参考】\n{}", intent_reference));
        }

        if !recent_window.is_empty() {
            let mut context_lines = Vec::new();
            for item in recent_window.iter().rev().take(6).rev() {
                let role = match item
                    .get("speaker_role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                {
                    "assistant" => "助手",
                    _ => "用户",
                };
                let text = item
                    .get("display_text")
                    .and_then(|v| v.as_str())
                    .or_else(|| item.get("text").and_then(|v| v.as_str()))
                    .unwrap_or("");
                if !text.is_empty() {
                    context_lines.push(format!("{}：{}", role, text));
                }
            }
            if !context_lines.is_empty() {
                parts.push(format!("【对话上下文】\n{}", context_lines.join("\n")));
            }
        } else if !context_parts.is_empty() {
            parts.push(format!("【最近上下文】\n{}", context_parts.join("\n")));
        }

        let user_content = parts.join("\n\n");

        let system_prompt = self.build_system_prompt().await?;

        let messages = vec![
            ChatMessage::text("system", system_prompt),
            ChatMessage::text("user", user_content),
        ];

        let request = ChatCompletionRequest {
            model: self.model_name.clone(),
            messages,
            temperature: Some(0.1),
            max_tokens: Some(256),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = match self.ai_client.chat_completion(&request).await {
            Ok(r) => r,
            Err(_) => {
                tracing::warn!("[表情服务] 表情包回复意图判断失败");
                return Ok(EmojiReplyDecision::default());
            }
        };

        let data = Self::extract_json_object(&response.content);
        let should_send = data
            .get("should_send")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let target_intent = data
            .get("target_intent")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tone = data
            .get("tone")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let emotion = data
            .get("emotion")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let intent = if !target_intent.is_empty() {
            target_intent
        } else if !tone.is_empty() && !emotion.is_empty() {
            format!("{}-{}", tone, emotion)
        } else {
            String::new()
        };

        let confidence = data
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        Ok(EmojiReplyDecision {
            should_send: should_send && !intent.is_empty(),
            emoji_file_ids: Vec::new(),
            reply_intent: if intent.is_empty() {
                None
            } else {
                Some(intent)
            },
            confidence,
        })
    }

    fn weighted_pick(
        &self,
        candidates: &[&crate::emoji::database::StickerRecord],
        group_id: &str,
    ) -> Option<String> {
        if candidates.is_empty() {
            return None;
        }

        let mut weights: Vec<f64> = candidates
            .iter()
            .map(|c| {
                let base = 1.0 / (1.0 + c.auto_reply_count as f64);
                base.max(0.01)
            })
            .collect();

        let cache = self.sent_emoji_cache.lock();
        if let Some(sent) = cache.get(group_id) {
            for (i, c) in candidates.iter().enumerate() {
                if sent.contains(&c.file_hash) {
                    weights[i] *= 0.1;
                }
            }
        }
        drop(cache);

        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            return Some(candidates[0].file_hash.clone());
        }

        let mut rng = rand::thread_rng();
        let mut r: f64 = rng.gen();
        for (i, w) in weights.iter().enumerate() {
            let prob = w / total;
            if r < prob {
                return Some(candidates[i].file_hash.clone());
            }
            r -= prob;
        }

        Some(candidates.last()?.file_hash.clone())
    }

    fn group_cooldown_active(&self, group_id: &str) -> bool {
        let state = self.cooldown_state.lock();
        if let Some(last) = state.get(group_id) {
            return last.elapsed().as_secs_f64() < self.config.reply_cooldown_seconds;
        }
        false
    }

    fn emoji_cooldown_active(&self, record: &crate::emoji::database::StickerRecord) -> bool {
        if record.last_auto_reply_at.is_empty() {
            return false;
        }
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&record.last_auto_reply_at) {
            let elapsed = chrono::Utc::now().signed_duration_since(ts).num_seconds() as f64;
            return elapsed < self.config.reply_cooldown_seconds;
        }
        false
    }

    async fn build_system_prompt(&self) -> XueliResult<String> {
        let template = self
            .template_loader
            .get_template("zh-CN", "emoji_reply.prompt")
            .await
            .unwrap_or_else(|_| {
                "你是一个表情选择助手。判断是否应该在助手回复后追加表情贴纸。\n只输出JSON：{{\"should_send\": false, \"tone\": \"\", \"emotion\": \"\", \"reason\": \"\"}}".to_string()
            });

        let emotion_labels = if self.config.emotion_labels.is_empty() {
            &self.default_emotion_labels
        } else {
            &self.config.emotion_labels
        };
        let reply_tones = &self.default_reply_tones;

        let emotion_str = emotion_labels.join(" / ");
        let tones_str = reply_tones.join(" / ");

        let vars: HashMap<&str, &str> = [
            ("emotion_labels", emotion_str.as_str()),
            ("reply_tones", tones_str.as_str()),
        ]
        .into_iter()
        .collect();

        Ok(self.template_loader.render(&template, &vars))
    }

    fn extract_json_object(content: &str) -> serde_json::Value {
        let text = content.trim();
        if text.is_empty() {
            return serde_json::Value::Object(Default::default());
        }
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(text) {
            if data.is_object() {
                return data;
            }
        }
        if let Some(fenced) = Self::extract_fenced_json(text) {
            return fenced;
        }
        if let Some(braced) = Self::extract_braced_json(text) {
            return braced;
        }
        serde_json::Value::Object(Default::default())
    }

    fn extract_fenced_json(text: &str) -> Option<serde_json::Value> {
        let re = regex::Regex::new(r"(?s)```(?:json)?\s*(\{.*?\})\s*```").ok()?;
        let caps = re.captures(text)?;
        let inner = caps.get(1)?.as_str();
        serde_json::from_str(inner).ok()
    }

    fn extract_braced_json(text: &str) -> Option<serde_json::Value> {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        if end <= start {
            return None;
        }
        let inner = &text[start..=end];
        serde_json::from_str(inner).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::prompt_loader::NoopPromptTemplateLoader;

    #[test]
    fn test_extract_json_direct() {
        let json = r#"{"should_send": true, "tone": "开心"}"#;
        let result = EmojiReplyService::<NoopPromptTemplateLoader>::extract_json_object(json);
        assert_eq!(result["should_send"].as_bool(), Some(true));
        assert_eq!(result["tone"].as_str(), Some("开心"));
    }

    #[test]
    fn test_extract_json_fenced() {
        let text = "好的，这是结果:\n```json\n{\"should_send\": false}\n```";
        let result = EmojiReplyService::<NoopPromptTemplateLoader>::extract_json_object(text);
        assert_eq!(result["should_send"].as_bool(), Some(false));
    }

    #[test]
    fn test_extract_json_braced() {
        let text = "前缀文本 {\"should_send\": true, \"emotion\": \"sad\"} 后缀文本";
        let result = EmojiReplyService::<NoopPromptTemplateLoader>::extract_json_object(text);
        assert_eq!(result["should_send"].as_bool(), Some(true));
    }

    #[test]
    fn test_emoji_reply_decision_default() {
        let d = EmojiReplyDecision::default();
        assert!(!d.should_send);
        assert_eq!(d.confidence, 0.0);
    }
}
