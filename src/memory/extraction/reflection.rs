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
            return Ok(ReflectionResult::default());
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
        Ok(ReflectionResult::default())
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
    min_topic_overlap: f64,
    candidate_limit: usize,
) -> Vec<std::collections::HashMap<String, serde_json::Value>> {
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
    let limit = candidate_limit.max(1);
    scored
        .into_iter()
        .take(limit)
        .map(|(_, entry)| entry)
        .collect()
}

pub fn build_reflection_evidence(
    anchor_turns: &[(String, String)],
    candidates: &[std::collections::HashMap<String, serde_json::Value>],
) -> Vec<std::collections::HashMap<String, serde_json::Value>> {
    use std::collections::HashMap;
    let mut evidence: Vec<HashMap<String, serde_json::Value>> = Vec::new();

    let user_quotes: Vec<String> = anchor_turns
        .iter()
        .filter_map(|(user, _)| {
            let t = user.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        })
        .take(3)
        .collect();
    let turn_label = if anchor_turns.len() == 1 {
        "T0"
    } else {
        "T0-T0"
    };

    let mut new_evidence = HashMap::new();
    new_evidence.insert("kind".to_string(), serde_json::json!("new_memory"));
    new_evidence.insert("turn_range".to_string(), serde_json::json!(turn_label));
    new_evidence.insert(
        "quote".to_string(),
        serde_json::json!(user_quotes.join(" / ")),
    );
    evidence.push(new_evidence);

    for candidate in candidates {
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
        evidence.push(entry);
    }

    evidence
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

        let records = vec![r1];
        let candidates = find_conflict_candidates("喜欢喝咖啡", &records, 0.1, 3);
        assert!(!candidates.is_empty());
        assert!(candidates[0].contains_key("score"));
    }

    #[test]
    fn test_build_reflection_evidence() {
        let turns = vec![("用户说喜欢咖啡".to_string(), "好的".to_string())];
        let candidates: Vec<std::collections::HashMap<String, serde_json::Value>> = vec![];
        let evidence = build_reflection_evidence(&turns, &candidates);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0]["kind"], "new_memory");
    }
}
