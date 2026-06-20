use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;

use crate::core::types::MemoryItem;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};

use super::two_stage_retriever::RetrievalContext;

/// 重排序候选文本最小长度限制
const MIN_RERANK_CANDIDATE_MAX_CHARS: usize = 32;
/// 重排序提示词最小总预算
const MIN_RERANK_TOTAL_PROMPT_BUDGET: usize = 512;

/// 重排序后的带分记忆
#[derive(Debug, Clone)]
pub struct ScoredMemory {
    /// 记忆条目
    pub memory: MemoryItem,
    /// 重排序得分
    pub score: f64,
}

/// 重排序器抽象
///
/// 输入 `(query, Vec<candidate_document>)`，输出按相关性排序的 `Vec<scored_document>`。
#[async_trait]
pub trait Reranker: Send + Sync {
    /// 对候选记忆进行重排序
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<(MemoryItem, f64)>,
        top_k: usize,
        context: Option<&RetrievalContext>,
    ) -> Vec<ScoredMemory>;
}

/// API 重排序器运行时配置
#[derive(Debug, Clone)]
pub struct APIRerankerConfig {
    /// 模型名称
    pub model: String,
    /// 单条候选最大字符数
    pub candidate_max_chars: usize,
    /// 提示词总字符预算
    pub total_prompt_budget: usize,
    /// 采样温度
    pub temperature: f64,
    /// 最大输出 token 数
    pub max_tokens: u32,
}

impl Default for APIRerankerConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            candidate_max_chars: 160,
            total_prompt_budget: 2400,
            temperature: 0.1,
            max_tokens: 1200,
        }
    }
}

/// 基于 LLM API 的重排序器
///
/// 通过 `AIClient` 调用外部重排序 API，system prompt 来自 `prompts/{locale}/rerank.prompt`，
/// 由调用方加载后传入。
pub struct APIReranker {
    client: Arc<dyn AIClient>,
    system_prompt: String,
    config: APIRerankerConfig,
}

impl APIReranker {
    /// 创建新的 API 重排序器
    ///
    /// - `client`：已配置好的 AI 客户端（负责请求 / 响应解析 / response_path）。
    /// - `system_prompt`：从 `PromptTemplateLoader` 加载的 rerank system prompt。
    /// - `config`：运行时配置。
    pub fn new(
        client: Arc<dyn AIClient>,
        system_prompt: impl Into<String>,
        mut config: APIRerankerConfig,
    ) -> Self {
        config.candidate_max_chars = config
            .candidate_max_chars
            .max(MIN_RERANK_CANDIDATE_MAX_CHARS);
        config.total_prompt_budget = config
            .total_prompt_budget
            .max(MIN_RERANK_TOTAL_PROMPT_BUDGET);
        Self {
            client,
            system_prompt: system_prompt.into(),
            config,
        }
    }

    fn build_user_prompt(
        &self,
        query: &str,
        candidates: &[(MemoryItem, f64)],
        top_k: usize,
        context: Option<&RetrievalContext>,
    ) -> String {
        let ctx = context.cloned().unwrap_or_default();
        let mut lines = vec![
            format!("query: {}", query),
            format!("top_k: {}", top_k),
            "request_context:".to_string(),
            format!("- requester_user_id={}", ctx.user_id),
            format!("- group_id={}", ctx.group_id),
            format!("- hour_of_day={}", ctx.hour_of_day),
            "candidates:".to_string(),
        ];

        let mut used: usize = lines.iter().map(|l| l.len() + 1).sum();
        for (mem, local_score) in candidates {
            let candidate_line = self.build_candidate_line(mem, *local_score);
            // 至少保留 8 行头信息后，按预算截断
            if lines.len() > 8 && used + candidate_line.len() + 1 > self.config.total_prompt_budget
            {
                break;
            }
            used += candidate_line.len() + 1;
            lines.push(candidate_line);
        }

        lines.join("\n")
    }

    fn build_candidate_line(&self, mem: &MemoryItem, local_score: f64) -> String {
        let content =
            self.truncate_candidate_content(&mem.content, self.config.candidate_max_chars);
        format!(
            "- id={id}; local_score={local:.4}; type={memory_type}; importance={importance}; \
             mention_count={mention_count}; owner_user_id={owner_user_id}; updated_at={updated_at}; content={content}",
            id = mem.id,
            local = local_score,
            memory_type = memory_type_name(&mem.memory_type),
            importance = mem.importance,
            mention_count = mem.access_count,
            owner_user_id = mem.user_id,
            updated_at = mem.last_accessed_at.to_rfc3339(),
            content = content,
        )
    }

    fn truncate_candidate_content(&self, content: &str, max_chars: usize) -> String {
        let normalized: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.len() <= max_chars {
            return normalized;
        }
        let cutoff = max_chars.saturating_sub(3).max(1);
        format!("{}...", &normalized[..cutoff].trim_end())
    }

    fn parse_response(&self, content: &str) -> Vec<(String, f64)> {
        let text = content.trim();
        let re =
            Regex::new(r"^```(?:json)?\s*|\s*```$").unwrap_or_else(|_| Regex::new("").unwrap());
        let text = re.replace_all(text, "").trim().to_string();

        let payload: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let items = payload
            .as_array()
            .cloned()
            .or_else(|| payload.get("results").and_then(|v| v.as_array()).cloned())
            .unwrap_or_default();

        items
            .into_iter()
            .filter_map(|item| {
                let obj = item.as_object()?;
                let id = obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if id.is_empty() {
                    return None;
                }
                let score = obj.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                Some((id, score))
            })
            .collect()
    }

    /// 关闭底层 AI 客户端（若客户端支持关闭语义）
    ///
    /// 当前 `AIClient` trait 未定义 `close`，因此此函数仅保留接口一致性占位。
    pub async fn close(&self) {
        // AIClient trait 没有 close 方法；依赖 Drop / reqwest 客户端内部回收。
    }
}

#[async_trait]
impl Reranker for APIReranker {
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<(MemoryItem, f64)>,
        top_k: usize,
        context: Option<&RetrievalContext>,
    ) -> Vec<ScoredMemory> {
        if candidates.is_empty() {
            return Vec::new();
        }

        let user_prompt = self.build_user_prompt(query, &candidates, top_k, context);
        let request = ChatCompletionRequest {
            model: self.config.model.clone(),
            messages: vec![
                ChatMessage::text("system", &self.system_prompt),
                ChatMessage::text("user", user_prompt),
            ],
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: HashMap::new(),
        };

        let ranked = match self.client.chat_completion(&request).await {
            Ok(response) => self.parse_response(&response.content),
            Err(e) => {
                tracing::error!("[检索] API 重排失败: {}", e);
                return candidates
                    .into_iter()
                    .take(top_k)
                    .map(|(memory, score)| ScoredMemory { memory, score })
                    .collect();
            }
        };

        if ranked.is_empty() {
            return candidates
                .into_iter()
                .take(top_k)
                .map(|(memory, score)| ScoredMemory { memory, score })
                .collect();
        }

        let id_to_memory: HashMap<String, MemoryItem> = candidates
            .into_iter()
            .map(|(mem, _)| (mem.id.clone(), mem))
            .collect();

        let mut ordered: Vec<ScoredMemory> = Vec::new();
        for (mem_id, score) in ranked {
            if let Some(memory) = id_to_memory.get(&mem_id) {
                ordered.push(ScoredMemory {
                    memory: memory.clone(),
                    score,
                });
            }
        }

        if ordered.is_empty() {
            return id_to_memory
                .into_values()
                .take(top_k)
                .map(|memory| ScoredMemory { memory, score: 0.0 })
                .collect();
        }

        ordered.truncate(top_k);
        ordered
    }
}

/// 本地 Cross-Encoder 重排序器占位
///
/// 当前未引入 ONNX / sentence-transformers 等本地模型运行时，
/// 因此仅作为 trait 占位实现，直接按输入顺序返回前 `top_k` 个候选。
/// 后续若引入 `ort` 等轻量 ONNX 推理库，可替换为真实 Cross-Encoder 推理。
pub struct CrossEncoderReranker;

impl CrossEncoderReranker {
    /// 创建本地 Cross-Encoder 占位重排序器
    pub fn new() -> Self {
        Self
    }
}

impl Default for CrossEncoderReranker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Reranker for CrossEncoderReranker {
    async fn rerank(
        &self,
        _query: &str,
        candidates: Vec<(MemoryItem, f64)>,
        top_k: usize,
        _context: Option<&RetrievalContext>,
    ) -> Vec<ScoredMemory> {
        candidates
            .into_iter()
            .take(top_k)
            .map(|(memory, score)| ScoredMemory { memory, score })
            .collect()
    }
}

fn memory_type_name(memory_type: &crate::core::types::MemoryType) -> &'static str {
    use crate::core::types::MemoryType;
    match memory_type {
        MemoryType::Fact => "fact",
        MemoryType::Preference => "preference",
        MemoryType::Event => "event",
        MemoryType::Opinion => "opinion",
        MemoryType::Relationship => "relationship",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{MemoryItem, MemoryType};

    fn make_memory(id: &str, user_id: &str, content: &str, importance: f64) -> MemoryItem {
        MemoryItem {
            id: id.to_string(),
            user_id: user_id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Fact,
            importance,
            created_at: chrono::Utc::now(),
            last_accessed_at: chrono::Utc::now(),
            access_count: 1,
        }
    }

    #[test]
    fn test_truncate_candidate_content() {
        let client = Arc::new(crate::services::ai_client::NoopAIClient);
        let reranker = APIReranker::new(
            client,
            "system",
            APIRerankerConfig {
                candidate_max_chars: 10,
                ..Default::default()
            },
        );

        assert_eq!(reranker.truncate_candidate_content("short", 10), "short");
        let long = "this is a very long content string";
        assert!(reranker
            .truncate_candidate_content(long, 10)
            .ends_with("..."));
        assert!(reranker.truncate_candidate_content(long, 10).len() <= 13);
    }

    #[test]
    fn test_parse_response_list() {
        let client = Arc::new(crate::services::ai_client::NoopAIClient);
        let reranker = APIReranker::new(client, "system", APIRerankerConfig::default());

        let content = r#"[{"id": "m1", "score": 0.9}, {"id": "m2", "score": 0.5}]"#;
        let parsed = reranker.parse_response(content);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], ("m1".to_string(), 0.9));
        assert_eq!(parsed[1], ("m2".to_string(), 0.5));
    }

    #[test]
    fn test_parse_response_wrapped() {
        let client = Arc::new(crate::services::ai_client::NoopAIClient);
        let reranker = APIReranker::new(client, "system", APIRerankerConfig::default());

        let content = "```json\n{\"results\": [{\"id\": \"m1\", \"score\": 0.95}]}\n```";
        let parsed = reranker.parse_response(content);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], ("m1".to_string(), 0.95));
    }

    #[tokio::test]
    async fn test_cross_encoder_placeholder() {
        let reranker = CrossEncoderReranker::new();
        let candidates = vec![
            (make_memory("m1", "u1", "咖啡", 0.5), 0.8),
            (make_memory("m2", "u1", "茶", 0.5), 0.6),
        ];
        let result = reranker.rerank("饮料", candidates, 1, None).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].memory.id, "m1");
    }
}
