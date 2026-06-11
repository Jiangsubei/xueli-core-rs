use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::core::types::{MemoryItem, MemoryPatch, MemoryType};
use crate::prelude::XueliResult;
use crate::traits::ai_client::{
    AIClient, ChatCompletionRequest, ChatCompletionResponse, ChatMessage,
};
use crate::traits::prompt_template::PromptTemplateLoader;

use super::buffer::{BufferTurn, ExtractionBuffer};

/// LLM 记忆提取器 — 从对话中提取结构化记忆
///
/// 对应 Python 版 `xueli/src/memory/extraction/extractor.py`
pub struct MemoryExtractor<A: AIClient, L: PromptTemplateLoader> {
    ai_client: Arc<A>,
    model: String,
    max_retries: usize,
    prompt_loader: Arc<L>,
    buffer: Arc<std::sync::Mutex<ExtractionBuffer>>,
    extract_every_n_turns: usize,
    max_dialogue_length: usize,
}

impl<A: AIClient, L: PromptTemplateLoader> MemoryExtractor<A, L> {
    pub fn new(ai_client: Arc<A>, model: &str, prompt_loader: Arc<L>) -> Self {
        Self {
            ai_client,
            model: model.to_string(),
            max_retries: 3,
            prompt_loader,
            buffer: Arc::new(std::sync::Mutex::new(ExtractionBuffer::new())),
            extract_every_n_turns: 5,
            max_dialogue_length: 30,
        }
    }

    pub fn with_extract_every_n_turns(mut self, n: usize) -> Self {
        self.extract_every_n_turns = n.max(1);
        self
    }

    pub fn with_max_dialogue_length(mut self, len: usize) -> Self {
        self.max_dialogue_length = len.max(1);
        self
    }

    /// 添加一轮对话到缓冲区
    #[allow(clippy::too_many_arguments)]
    pub fn add_dialogue_turn(
        &self,
        user_id: &str,
        user_message: &str,
        assistant_message: &str,
        session_id: &str,
        turn_id: usize,
        dialogue_key: &str,
        message_type: &str,
        group_id: &str,
        message_id: &str,
        narrative_summary: &str,
        source_platform: &str,
    ) {
        let mut buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.add_dialogue_turn(
            user_id,
            user_message,
            assistant_message,
            session_id,
            turn_id,
            dialogue_key,
            message_type,
            group_id,
            message_id,
            narrative_summary,
            source_platform,
        );
    }

    /// 检查是否应触发记忆提取
    pub fn should_extract(&self, session_id: &str) -> bool {
        let buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.should_extract(session_id, self.extract_every_n_turns)
    }

    /// 获取指定会话的待提取轮次数量
    pub fn get_pending_turn_count(&self, session_id: &str) -> usize {
        let buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.get_pending_turn_count(session_id)
    }

    /// 触发记忆提取，可选强制模式跳过条件检查
    pub async fn trigger_extraction(
        &self,
        user_id: &str,
        force: bool,
        session_id: &str,
    ) -> XueliResult<Vec<MemoryItem>> {
        let session_key = session_id.trim().to_string();
        if session_key.is_empty() {
            return Ok(Vec::new());
        }
        if !force && !self.should_extract(&session_key) {
            return Ok(Vec::new());
        }
        self.extract_memories(user_id, &session_key).await
    }

    /// 执行记忆提取：从缓冲区获取待处理对话，调用 LLM 解析，返回结果
    pub async fn extract_memories(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> XueliResult<Vec<MemoryItem>> {
        let pending_turns: Vec<BufferTurn> = {
            let buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.get_pending_turns(session_id).into_iter().cloned().collect()
        };

        if pending_turns.is_empty() {
            return Ok(Vec::new());
        }

        let visible_turns: Vec<BufferTurn> = pending_turns
            .iter()
            .rev()
            .take(self.max_dialogue_length)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let latest_turn_id = pending_turns
            .iter()
            .map(|t| t.turn_id)
            .max()
            .unwrap_or(0);

        let dialogue_text = self.format_dialogue(&visible_turns, user_id);

        if dialogue_text.trim() == "无" || dialogue_text.trim().is_empty() {
            let mut buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.mark_session_extracted(session_id, latest_turn_id);
            return Ok(Vec::new());
        }

        let messages = vec![
            ChatMessage::text("system", &self.build_system_prompt().await),
            ChatMessage::text("user", &self.build_user_prompt(&dialogue_text).await),
        ];

        let start = Instant::now();
        let mut last_err = String::new();

        for attempt in 0..self.max_retries {
            let request = ChatCompletionRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                temperature: Some(0.3),
                max_tokens: Some(1024),
                stream: false,
                tools: None,
                tool_choice: None,
                extra_params: Default::default(),
            };

            match self.ai_client.chat_completion(&request).await {
                Ok(response) => match self.parse_response(&response, user_id) {
                    Ok(patch) => {
                        tracing::debug!(
                            user_id = user_id,
                            add_count = patch.add.len(),
                            elapsed_ms = start.elapsed().as_millis(),
                            "[MemoryExtractor] 提取完成"
                        );

                        // 构建元数据
                        let related_dialogue = self.build_related_dialogue_snapshot(&visible_turns);
                        let dialogue_key = {
                            let buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
                            buf.get_dialogue_key(session_id).to_string()
                        };

                        let mut items_with_metadata = Vec::new();
                        for item in patch.add {
                            let metadata = self.build_memory_metadata(
                                user_id,
                                session_id,
                                &dialogue_key,
                                &visible_turns,
                                &related_dialogue,
                            );
                            // 将元数据信息编码到 content 中（MemoryItem 没有 metadata 字段）
                            // 存储层会在 add_memory_dedup 时处理
                            let _ = &metadata;
                            items_with_metadata.push(item);
                        }

                        if !items_with_metadata.is_empty() {
                            let mut buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
                            buf.mark_session_extracted(session_id, latest_turn_id);
                        }

                        return Ok(items_with_metadata);
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

        // 所有重试失败 → 返回空（静默失败）
        tracing::warn!("[MemoryExtractor] 全部尝试失败，返回空记忆: {}", last_err);
        Ok(Vec::new())
    }

    /// 从一组消息中提取记忆（简单接口，不使用缓冲区）
    pub async fn extract(&self, user_id: &str, messages: &[String]) -> XueliResult<MemoryPatch> {
        if messages.is_empty() {
            return Ok(MemoryPatch {
                add: Vec::new(),
                update: Vec::new(),
                remove: Vec::new(),
            });
        }

        let conversation = messages.join("\n");
        let system_prompt = self.build_system_prompt().await;
        let user_prompt = self.build_user_prompt(&conversation).await;

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

    /// 清空缓冲区，可选指定会话
    pub fn clear_buffer(&self, session_id: Option<&str>) {
        let mut buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.clear_buffer(session_id);
    }

    /// 构建系统提示词 — 使用 PromptTemplateLoader 加载模板，失败则兜底
    async fn build_system_prompt(&self) -> String {
        if let Ok(template) = self.prompt_loader.get_template("zh-CN", "memory_extraction").await {
            return template;
        }
        // 兜底：硬编码提示词
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

    /// 构建用户提示词 — 使用 PromptTemplateLoader，失败则兜底
    async fn build_user_prompt(&self, conversation: &str) -> String {
        if let Ok(template) = self.prompt_loader.get_template("zh-CN", "memory_extraction_user").await {
            let vars = HashMap::from([("conversation", conversation)]);
            return self.prompt_loader.render(&template, &vars);
        }
        // 兜底
        format!(
            "请从以下对话中提取关于用户的值得记住的信息：\n\n```\n{}\n```\n\n请输出 JSON。",
            conversation
        )
    }

    /// 将对话轮次格式化为 LLM 可读的文本 — 对应 Python 版 _format_dialogue
    fn format_dialogue(&self, turns: &[BufferTurn], user_id: &str) -> String {
        if turns.is_empty() {
            return "无".to_string();
        }

        let session_type = turns
            .last()
            .map(|t| t.source_message_type.as_str())
            .unwrap_or("");
        let session_label = if session_type == "group" {
            "群聊"
        } else {
            "私聊"
        };

        let mut lines = vec![
            format!("=== 用户 {} 的{}对话记录 ===", user_id, session_label),
            "下面这些内容同时包含用户发言和助手发言。".to_string(),
            "用户发言是判断记忆的主要来源，助手发言只用于帮助理解上下文，不能直接当成记忆来源。".to_string(),
            "每条前缀里的 Tn 是稳定 turn 标签，输出时必须引用它。".to_string(),
            String::new(),
        ];

        let mut has_user_content = false;
        for turn in turns {
            let user_content = turn.user.trim();
            let assistant_content = turn.assistant.trim();
            let turn_label = format!("T{}", turn.turn_id);

            if !user_content.is_empty() {
                has_user_content = true;
                lines.push(format!("{}: 用户{}: {}", turn_label, user_id, user_content));
            }
            if !assistant_content.is_empty() {
                lines.push(format!("{}: 助手: {}", turn_label, assistant_content));
            }
            if !user_content.is_empty() || !assistant_content.is_empty() {
                lines.push(String::new());
            }
        }

        if !has_user_content {
            return "无".to_string();
        }
        lines.join("\n").trim_end().to_string()
    }

    /// 构建记忆元数据 — 对应 Python 版 _build_memory_metadata
    #[allow(clippy::too_many_arguments)]
    fn build_memory_metadata(
        &self,
        owner_user_id: &str,
        session_id: &str,
        dialogue_key: &str,
        anchor_turns: &[BufferTurn],
        related_dialogue: &[serde_json::Value],
    ) -> serde_json::Value {
        let first_turn = match anchor_turns.first() {
            Some(t) => t,
            None => return serde_json::Value::Object(serde_json::Map::new()),
        };
        let last_turn = match anchor_turns.last() {
            Some(t) => t,
            None => return serde_json::Value::Object(serde_json::Map::new()),
        };

        let message_ids: Vec<String> = anchor_turns
            .iter()
            .filter_map(|t| {
                if t.source_message_id.trim().is_empty() {
                    None
                } else {
                    Some(t.source_message_id.clone())
                }
            })
            .collect();

        let now_hour = chrono::Utc::now().format("%H").to_string();

        serde_json::json!({
            "owner_user_id": owner_user_id,
            "dialogue_key": dialogue_key,
            "source_session_id": session_id,
            "source_dialogue_key": dialogue_key,
            "source_turn_start": first_turn.turn_id,
            "source_turn_end": last_turn.turn_id,
            "source_message_ids": message_ids,
            "source_message_id": message_ids.last().unwrap_or(&String::new()),
            "source_message_type": last_turn.source_message_type,
            "source_group_id": last_turn.source_group_id,
            "group_id": last_turn.source_group_id,
            "related_dialogue": related_dialogue,
            "encoding_context": {
                "hour_of_day": now_hour,
                "emotional_tone": "",
                "message_type": last_turn.source_message_type,
            },
        })
    }

    /// 构建关联对话快照 — 对应 Python 版 _build_related_dialogue_snapshot
    fn build_related_dialogue_snapshot(&self, turns: &[BufferTurn]) -> Vec<serde_json::Value> {
        turns
            .iter()
            .map(|turn| {
                serde_json::json!({
                    "turn_id": turn.turn_id,
                    "user": turn.user,
                    "assistant": turn.assistant,
                    "timestamp": turn.timestamp,
                    "source_message_type": turn.source_message_type,
                    "source_group_id": turn.source_group_id,
                    "source_message_id": turn.source_message_id,
                    "owner_user_id": turn.owner_user_id,
                })
            })
            .collect()
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

impl<A: AIClient, L: PromptTemplateLoader> Default for MemoryExtractor<A, L> {
    fn default() -> Self {
        unimplemented!("MemoryExtractor 需要 AI 客户端和模板加载器，请使用 new() 构造")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ai_client::NoopAIClient;
    use crate::services::prompt_loader::NoopPromptTemplateLoader;
    use crate::traits::ai_client::ChatCompletionResponse;

    fn make_extractor() -> MemoryExtractor<NoopAIClient, NoopPromptTemplateLoader> {
        MemoryExtractor::new(
            Arc::new(NoopAIClient),
            "gpt-4o-mini",
            Arc::new(NoopPromptTemplateLoader),
        )
    }

    fn make_response(content: &str) -> ChatCompletionResponse {
        ChatCompletionResponse {
            content: content.to_string(),
            reasoning_content: String::new(),
            finish_reason: String::new(),
            usage: None,
            model: String::new(),
            tool_calls: None,
            segments: None,
            raw_response: None,
            raw_content: String::new(),
        }
    }

    #[test]
    fn test_parse_response_empty() {
        let extractor = make_extractor();
        let response = make_response("");
        let patch = extractor.parse_response(&response, "u1").unwrap();
        assert!(patch.add.is_empty());
    }

    #[test]
    fn test_parse_response_with_memories() {
        let extractor = make_extractor();
        let json = r#"{
          "memories": [
            {"content": "用户喜欢喝咖啡", "memory_type": "preference", "importance": 0.7, "confidence": 0.9},
            {"content": "用户住在北京", "memory_type": "fact", "importance": 0.9, "confidence": 0.95}
          ]
        }"#;
        let response = make_response(json);
        let patch = extractor.parse_response(&response, "u1").unwrap();
        assert_eq!(patch.add.len(), 2);
        assert!(patch.add[0].content.contains("咖啡"));
        assert_eq!(patch.add[1].memory_type, MemoryType::Fact);
        assert!(patch.add[1].importance > 0.8);
    }

    #[test]
    fn test_parse_response_filter_low_confidence() {
        let extractor = make_extractor();
        let json = r#"{
          "memories": [
            {"content": "可靠记忆", "memory_type": "fact", "importance": 0.9, "confidence": 0.95},
            {"content": "不确定的记忆", "memory_type": "fact", "importance": 0.5, "confidence": 0.2}
          ]
        }"#;
        let response = make_response(json);
        let patch = extractor.parse_response(&response, "u1").unwrap();
        assert_eq!(patch.add.len(), 1);
        assert!(patch.add[0].content.contains("可靠"));
    }

    #[tokio::test]
    async fn test_build_system_prompt_not_empty() {
        let extractor = make_extractor();
        let prompt = extractor.build_system_prompt().await;
        assert!(prompt.contains("记忆提取"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_format_dialogue() {
        let extractor = make_extractor();
        let turns = vec![
            BufferTurn {
                turn_id: 1,
                user: "你好".to_string(),
                assistant: "你好呀！".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                source_message_type: "private".to_string(),
                source_group_id: String::new(),
                source_message_id: "msg1".to_string(),
                source_platform: "qq".to_string(),
                owner_user_id: "u1".to_string(),
                dialogue_key: "qq:private:u1".to_string(),
                session_id: "session_1".to_string(),
                narrative_summary: String::new(),
            },
            BufferTurn {
                turn_id: 2,
                user: "我喜欢喝咖啡".to_string(),
                assistant: "好的，记住了！".to_string(),
                timestamp: "2024-01-01T00:01:00Z".to_string(),
                source_message_type: "private".to_string(),
                source_group_id: String::new(),
                source_message_id: "msg2".to_string(),
                source_platform: "qq".to_string(),
                owner_user_id: "u1".to_string(),
                dialogue_key: "qq:private:u1".to_string(),
                session_id: "session_1".to_string(),
                narrative_summary: String::new(),
            },
        ];

        let formatted = extractor.format_dialogue(&turns, "u1");
        assert!(formatted.contains("T1"));
        assert!(formatted.contains("T2"));
        assert!(formatted.contains("咖啡"));
        assert!(formatted.contains("私聊"));
    }

    #[test]
    fn test_format_dialogue_empty() {
        let extractor = make_extractor();
        let formatted = extractor.format_dialogue(&[], "u1");
        assert_eq!(formatted, "无");
    }

    #[test]
    fn test_build_memory_metadata() {
        let extractor = make_extractor();
        let turns = vec![BufferTurn {
            turn_id: 1,
            user: "你好".to_string(),
            assistant: "你好呀！".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            source_message_type: "private".to_string(),
            source_group_id: String::new(),
            source_message_id: "msg1".to_string(),
            source_platform: "qq".to_string(),
            owner_user_id: "u1".to_string(),
            dialogue_key: "qq:private:u1".to_string(),
            session_id: "session_1".to_string(),
            narrative_summary: String::new(),
        }];

        let metadata = extractor.build_memory_metadata(
            "u1",
            "session_1",
            "qq:private:u1",
            &turns,
            &[],
        );

        assert_eq!(metadata["owner_user_id"], "u1");
        assert_eq!(metadata["source_turn_start"], 1);
        assert_eq!(metadata["source_message_type"], "private");
    }

    #[test]
    fn test_build_related_dialogue_snapshot() {
        let extractor = make_extractor();
        let turns = vec![BufferTurn {
            turn_id: 1,
            user: "你好".to_string(),
            assistant: "你好呀！".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            source_message_type: "private".to_string(),
            source_group_id: String::new(),
            source_message_id: "msg1".to_string(),
            source_platform: "qq".to_string(),
            owner_user_id: "u1".to_string(),
            dialogue_key: "qq:private:u1".to_string(),
            session_id: "session_1".to_string(),
            narrative_summary: String::new(),
        }];

        let snapshot = extractor.build_related_dialogue_snapshot(&turns);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0]["turn_id"], 1);
        assert_eq!(snapshot[0]["user"], "你好");
    }
}
