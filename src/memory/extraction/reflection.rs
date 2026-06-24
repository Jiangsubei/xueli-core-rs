use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use super::models::ExtractionConfig;
use crate::core::types::MemoryItem;
use crate::prelude::XueliResult;
use crate::traits::ai_client::{
    AIClient, ChatCompletionRequest, ChatCompletionResponse, ChatMessage,
};
use crate::traits::prompt_template::PromptTemplateLoader;

/// 记忆冲突反思 — 检测新旧记忆矛盾并给出解决方案
///
/// 对应 Python 版 `xueli/src/memory/extraction/reflection.py`
pub struct MemoryReflection<A: AIClient + ?Sized, L: PromptTemplateLoader> {
    ai_client: Arc<A>,
    model: String,
    max_retries: usize,
    prompt_loader: Arc<L>,
}

impl<A: AIClient + ?Sized, L: PromptTemplateLoader> MemoryReflection<A, L> {
    pub fn new(ai_client: Arc<A>, model: &str, prompt_loader: Arc<L>) -> Self {
        Self {
            ai_client,
            model: model.to_string(),
            max_retries: 3,
            prompt_loader,
        }
    }

    pub async fn reflect(
        &self,
        existing: &[MemoryItem],
        new_items: &[MemoryItem],
    ) -> XueliResult<ReflectionResult> {
        if existing.is_empty() || new_items.is_empty() {
            return Ok(ReflectionResult::default());
        }

        let system_prompt = self.build_system_prompt().await?;
        let user_prompt = self.build_user_prompt(existing, new_items).await?;

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
        Ok(ReflectionResult::default())
    }

    async fn build_system_prompt(&self) -> XueliResult<String> {
        self.prompt_loader.get_template("zh-CN", "reflection").await
    }

    async fn build_user_prompt(
        &self,
        existing: &[MemoryItem],
        new_items: &[MemoryItem],
    ) -> XueliResult<String> {
        let template = self
            .prompt_loader
            .get_template("zh-CN", "reflection_user")
            .await?;
        let old_list: Vec<String> = existing
            .iter()
            .map(|m| format!("- [{}] {}", m.id, m.content))
            .collect();
        let new_list: Vec<String> = new_items
            .iter()
            .map(|m| format!("- [{}] {}", m.id, m.content))
            .collect();
        let existing_memories = old_list.join("\n");
        let new_memories = new_list.join("\n");
        let vars = HashMap::from([
            ("existing_memories", existing_memories.as_str()),
            ("new_memories", new_memories.as_str()),
        ]);
        Ok(self.prompt_loader.render(&template, &vars))
    }

    fn parse_response(&self, response: &ChatCompletionResponse) -> XueliResult<ReflectionResult> {
        let text = response.content.trim();
        if text.is_empty() {
            return Ok(ReflectionResult::default());
        }

        let json_str = if let Some(start) = text.find('{') {
            let end = text.rfind('}').unwrap_or(text.len() - 1);
            &text[start..=end]
        } else {
            return Ok(ReflectionResult::default());
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
        let has_conflict = !conflicts.is_empty();

        let (conflict_type, action, summary, reason, confidence) = if let Some(first) = parsed
            .get("conflicts")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
        {
            (
                first
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                first
                    .get("resolution")
                    .and_then(|v| v.as_str())
                    .unwrap_or("keep_both")
                    .to_string(),
                parsed
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                first
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                parsed
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.5),
            )
        } else {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                0.5,
            )
        };

        Ok(ReflectionResult {
            conflicts,
            resolutions,
            has_conflict,
            conflict_type,
            action,
            summary,
            reason,
            confidence,
            evidence: Vec::new(),
            targets: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReflectionResult {
    pub conflicts: Vec<String>,
    pub resolutions: Vec<String>,
    pub has_conflict: bool,
    pub conflict_type: String,
    pub action: String,
    pub summary: String,
    pub reason: String,
    pub confidence: f64,
    pub evidence: Vec<std::collections::HashMap<String, serde_json::Value>>,
    pub targets: Vec<std::collections::HashMap<String, serde_json::Value>>,
}

const REFLECTION_NEGATIVE_CUES: &[&str] = &[
    "不喜欢",
    "讨厌",
    "不想",
    "不要",
    "不喝",
    "不吃",
    "不再",
    "不愿意",
    "拒绝",
    "别再",
    "不能接受",
    "没兴趣",
];

const REFLECTION_STANCE_TERMS: &[&str] = &[
    "喜欢",
    "讨厌",
    "不喜欢",
    "不想",
    "想",
    "想要",
    "不要",
    "愿意",
    "不愿意",
    "爱",
    "不爱",
    "偏好",
    "习惯",
    "最近",
    "现在",
    "目前",
    "今天",
    "这几天",
    "暂时",
    "先",
    "喝",
    "吃",
    "用",
];

const REFLECTION_TEMPORAL_MARKERS: &[&str] = &["最近", "现在", "目前", "今天", "这几天", "暂时"];

pub fn strip_stance_terms(text: &str) -> String {
    let mut result = text.to_string();
    for term in REFLECTION_STANCE_TERMS {
        result = result.replace(term, "");
    }
    result
        .chars()
        .filter(|c| c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
        .collect()
}

pub fn topic_overlap(left: &str, right: &str) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let left_chars: std::collections::HashSet<char> = left.chars().collect();
    let right_chars: std::collections::HashSet<char> = right.chars().collect();
    if left_chars.is_empty() || right_chars.is_empty() {
        return 0.0;
    }
    let intersection = left_chars.intersection(&right_chars).count();
    intersection as f64 / left_chars.len().min(right_chars.len()) as f64
}

pub fn has_negative_stance(text: &str) -> bool {
    REFLECTION_NEGATIVE_CUES
        .iter()
        .any(|cue| text.contains(cue))
}

pub fn estimate_conflict_score(left: &str, right: &str, min_topic_overlap: f64) -> f64 {
    let normalized_left = left.trim().to_lowercase();
    let normalized_right = right.trim().to_lowercase();
    if normalized_left.is_empty()
        || normalized_right.is_empty()
        || normalized_left == normalized_right
    {
        return 0.0;
    }

    let topic_left = strip_stance_terms(&normalized_left);
    let topic_right = strip_stance_terms(&normalized_right);
    let overlap = topic_overlap(&topic_left, &topic_right);
    if overlap < min_topic_overlap {
        return 0.0;
    }

    let left_neg = has_negative_stance(&normalized_left);
    let right_neg = has_negative_stance(&normalized_right);
    if left_neg == right_neg {
        return 0.0;
    }

    let temporal_bonus = if REFLECTION_TEMPORAL_MARKERS
        .iter()
        .any(|m| normalized_left.contains(m) || normalized_right.contains(m))
    {
        0.1
    } else {
        0.0
    };

    (overlap + 0.25 + temporal_bonus).min(1.0)
}

pub fn find_conflict_candidates(
    content: &str,
    existing_records: &[std::collections::HashMap<String, serde_json::Value>],
    config: &ExtractionConfig,
) -> Vec<std::collections::HashMap<String, serde_json::Value>> {
    let min_topic_overlap = config.reflection_min_topic_overlap;
    let candidate_limit = config.reflection_candidate_limit.max(1);
    let mut scored: Vec<(f64, std::collections::HashMap<String, serde_json::Value>)> = Vec::new();
    for record in existing_records {
        let existing_content = record
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if existing_content.is_empty() {
            continue;
        }
        let score = estimate_conflict_score(content, existing_content, min_topic_overlap);
        if score <= 0.0 {
            continue;
        }
        let mut entry = record.clone();
        entry.insert("score".to_string(), serde_json::json!(score));
        scored.push((score, entry));
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(candidate_limit)
        .map(|(_, entry)| entry)
        .collect()
}

pub fn build_reflection_evidence(
    anchor_turns: &[std::collections::HashMap<String, serde_json::Value>],
    candidates: &[std::collections::HashMap<String, serde_json::Value>],
) -> Vec<std::collections::HashMap<String, serde_json::Value>> {
    use std::collections::HashMap;
    let mut evidence: Vec<HashMap<String, serde_json::Value>> = Vec::new();

    let turn_start = anchor_turns
        .first()
        .and_then(|t| t.get("turn_id"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let turn_end = anchor_turns
        .last()
        .and_then(|t| t.get("turn_id"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let user_quotes: Vec<String> = anchor_turns
        .iter()
        .filter_map(|turn| {
            let user = turn.get("user").and_then(|v| v.as_str()).unwrap_or("");
            let t = user.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        })
        .collect();

    let turn_range = if turn_start == turn_end {
        format!("T{}", turn_start)
    } else {
        format!("T{}-T{}", turn_start, turn_end)
    };

    let last_turn = anchor_turns.last();

    let mut new_entry = HashMap::new();
    new_entry.insert("kind".to_string(), serde_json::json!("new_memory"));
    new_entry.insert("turn_range".to_string(), serde_json::json!(turn_range));
    new_entry.insert(
        "source_session_id".to_string(),
        serde_json::json!(last_turn
            .and_then(|t| t.get("session_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")),
    );
    new_entry.insert(
        "source_group_id".to_string(),
        serde_json::json!(last_turn
            .and_then(|t| t.get("source_group_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")),
    );
    new_entry.insert(
        "quote".to_string(),
        serde_json::json!(user_quotes[..user_quotes.len().min(3)].join(" / ")),
    );
    evidence.push(new_entry);

    for candidate in candidates {
        let metadata = candidate
            .get("metadata")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let mut entry = HashMap::new();
        entry.insert("kind".to_string(), serde_json::json!("existing_memory"));
        entry.insert(
            "memory_id".to_string(),
            candidate
                .get("id")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
        entry.insert(
            "memory_type".to_string(),
            candidate
                .get("kind")
                .cloned()
                .unwrap_or(serde_json::json!("ordinary")),
        );
        entry.insert(
            "content".to_string(),
            candidate
                .get("content")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
        entry.insert(
            "source_session_id".to_string(),
            metadata
                .get("source_session_id")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
        entry.insert(
            "source_turn_start".to_string(),
            serde_json::json!(metadata
                .get("source_turn_start")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)),
        );
        entry.insert(
            "source_turn_end".to_string(),
            serde_json::json!(metadata
                .get("source_turn_end")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)),
        );
        entry.insert(
            "source_group_id".to_string(),
            metadata
                .get("source_group_id")
                .or_else(|| metadata.get("group_id"))
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        );
        evidence.push(entry);
    }

    evidence
}

impl<A: AIClient + Default, L: PromptTemplateLoader + Default> Default for MemoryReflection<A, L> {
    fn default() -> Self {
        tracing::warn!(
            "[MemoryReflection] 使用 Default 构造，AI 客户端和模板加载器均为默认值，生产环境请使用 new()"
        );
        Self::new(
            Arc::new(A::default()),
            "gpt-4o-mini",
            Arc::new(L::default()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ai_client::NoopAIClient;
    use crate::services::prompt_loader::FilePromptTemplateLoader;
    use crate::traits::ai_client::ChatCompletionResponse;

    fn file_loader() -> Arc<FilePromptTemplateLoader> {
        let base = std::path::PathBuf::from(std::env!("CARGO_MANIFEST_DIR")).join("prompts");
        Arc::new(FilePromptTemplateLoader::new(base))
    }

    fn make_reflection() -> MemoryReflection<NoopAIClient, FilePromptTemplateLoader> {
        MemoryReflection::new(Arc::new(NoopAIClient), "gpt-4o-mini", file_loader())
    }

    #[test]
    fn test_parse_response_with_conflicts() {
        let reflection = make_reflection();
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
            raw_content: String::new(),
            raw_response: None,
        };
        let result = reflection.parse_response(&response).unwrap();
        assert_eq!(result.conflicts.len(), 1);
        assert!(result.conflicts[0].contains("update_old"));
    }

    #[test]
    fn test_parse_response_no_conflicts() {
        let reflection = make_reflection();
        let json = r#"{"conflicts": []}"#;
        let response = ChatCompletionResponse {
            content: json.to_string(),
            reasoning_content: "".to_string(),
            finish_reason: "".to_string(),
            usage: None,
            model: "".to_string(),
            tool_calls: None,
            segments: None,
            raw_content: String::new(),
            raw_response: None,
        };
        let result = reflection.parse_response(&response).unwrap();
        assert!(result.conflicts.is_empty());
    }

    #[test]
    fn test_strip_stance_terms() {
        let result = strip_stance_terms("最近不喜欢喝咖啡");
        assert!(!result.is_empty());
        assert!(!result.contains("最近"));
        assert!(!result.contains("喝"));
    }

    #[test]
    fn test_topic_overlap() {
        assert!(topic_overlap("喜欢喝咖啡", "讨厌喝咖啡") > 0.3);
        assert_eq!(topic_overlap("咖啡", "旅游"), 0.0);
        assert_eq!(topic_overlap("", ""), 0.0);
    }

    #[test]
    fn test_has_negative_stance() {
        assert!(has_negative_stance("我不喜欢这个"));
        assert!(has_negative_stance("讨厌下雨"));
        assert!(!has_negative_stance("我喜欢晴天"));
    }

    #[test]
    fn test_estimate_conflict_score_same_topic_polarity_diff() {
        let score = estimate_conflict_score("喜欢喝咖啡", "讨厌喝咖啡", 0.1);
        assert!(score > 0.0);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_estimate_conflict_score_same_stance_no_conflict() {
        let score = estimate_conflict_score("喜欢咖啡", "喜欢奶茶", 0.45);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_estimate_conflict_score_empty() {
        assert_eq!(estimate_conflict_score("", "", 0.1), 0.0);
        assert_eq!(estimate_conflict_score("hello", "hello", 0.1), 0.0);
    }

    #[test]
    fn test_find_conflict_candidates() {
        use std::collections::HashMap;
        let mut r1 = HashMap::new();
        r1.insert("id".to_string(), serde_json::json!("id1"));
        r1.insert("content".to_string(), serde_json::json!("讨厌喝咖啡"));
        r1.insert("kind".to_string(), serde_json::json!("ordinary"));

        let config = ExtractionConfig {
            reflection_min_topic_overlap: 0.1,
            reflection_candidate_limit: 3,
            ..Default::default()
        };

        let records = vec![r1];
        let candidates = find_conflict_candidates("喜欢喝咖啡", &records, &config);
        assert!(!candidates.is_empty());
        assert!(candidates[0].contains_key("score"));
    }

    #[test]
    fn test_build_reflection_evidence() {
        let mut turn = std::collections::HashMap::new();
        turn.insert("turn_id".to_string(), serde_json::json!(1));
        turn.insert("user".to_string(), serde_json::json!("用户说喜欢咖啡"));
        turn.insert("session_id".to_string(), serde_json::json!("s1"));
        let turns = vec![turn];
        let candidates: Vec<std::collections::HashMap<String, serde_json::Value>> = vec![];
        let evidence = build_reflection_evidence(&turns, &candidates);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0]["kind"], "new_memory");
    }
}
