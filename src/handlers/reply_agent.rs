use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;

use crate::prelude::XueliResult;
use crate::core::log_labels::{LOG_PROMPT_DIGEST, LOG_RETRY};
use crate::core::platform_types::InboundEvent;
use crate::core::scope::ChatScope;
use crate::handlers::context_builder::ConversationContext;
use crate::handlers::prompt_builder::ReplyPromptBuilder;
use crate::memory::manager::MemoryManager;
use crate::memory::stores::person_fact::SqlitePersonFactStore;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage, MessageContent};
use crate::traits::prompt_template::PromptTemplateLoader;

/// 工具定义
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 可执行工具 trait
#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    fn is_end_tool(&self) -> bool {
        false
    }
    async fn execute(&self, args: serde_json::Value) -> XueliResult<String>;
}

// ── reply 工具 ───────────────────────────────────────────

/// 内置 reply 工具 — 发送可见回复，调用后本轮结束
struct ReplyTool;

#[async_trait]
impl Tool for ReplyTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "reply".to_string(),
            description: "发送一条可见回复给用户。调用此工具后本轮对话结束。".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "回复的完整文本内容"
                    },
                    "segments": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "可选。将回复拆成多段顺序发送，适合长回复自然分段。不填则作为整体发送。"
                    },
                    "expected_effect": {
                        "type": "string",
                        "enum": ["continue", "satisfy", "cool_down", "clarify"],
                        "description": "预期这条回复产生的效果；无法判断时不要填写。"
                    }
                },
                "required": ["text"]
            }),
        }
    }

    fn is_end_tool(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> XueliResult<String> {
        let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
        Ok(text.to_string())
    }
}

// ── query_memory 工具 ────────────────────────────────────

/// 查询记忆工具 — 搜索与当前用户相关的记忆
struct QueryMemoryTool {
    memory_manager: Arc<MemoryManager>,
    user_id: String,
}

#[async_trait]
impl Tool for QueryMemoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "query_memory".to_string(),
            description: "查询关于当前对话用户的相关记忆。返回匹配的记忆条目。".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "要查询的关键词或问题"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> XueliResult<String> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");

        if query.is_empty() {
            return Ok("（未提供查询关键词）".to_string());
        }

        // 搜索记忆
        let results = self
            .memory_manager
            .search(query, 5)
            .await
            .unwrap_or_default();

        if results.is_empty() {
            Ok(format!("未找到与「{query}」相关的记忆。"))
        } else {
            let items: Vec<String> = results
                .iter()
                .map(|m| format!("- [{:?}] {}", m.memory_type, m.content))
                .collect();
            Ok(format!(
                "找到 {} 条相关记忆：\n{}",
                results.len(),
                items.join("\n")
            ))
        }
    }
}

// ── query_person 工具 ────────────────────────────────────

/// 查询人物档案工具 — 查询用户的人物事实
struct QueryPersonTool {
    person_fact_store: Arc<SqlitePersonFactStore>,
    user_id: String,
}

#[async_trait]
impl Tool for QueryPersonTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "query_person".to_string(),
            description: "查询特定用户的人物档案，包括关系状态、风格偏好、近期情绪趋势等。"
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "用户名、昵称或用户ID"
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> XueliResult<String> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.user_id);

        let facts = self
            .person_fact_store
            .get_by_user(name)
            .await
            .unwrap_or_default();

        if facts.is_empty() {
            Ok(format!("未找到关于「{name}」的人物档案。"))
        } else {
            let items: Vec<String> = facts
                .iter()
                .map(|f| {
                    format!(
                        "- [{}] {} (置信度: {:.0}%)",
                        f.category,
                        f.fact_text,
                        f.confidence * 100.0
                    )
                })
                .collect();
            Ok(format!("关于「{name}」的档案：\n{}", items.join("\n")))
        }
    }
}

// ── view_message 工具 ────────────────────────────────────

/// 查看消息工具 — 展开复杂消息（如合并转发消息）
struct ViewMessageTool;

#[async_trait]
impl Tool for ViewMessageTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "view_message".to_string(),
            description: "展开一条复杂消息的完整内容，例如合并转发消息或卡片消息。".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "msg_id": {
                        "type": "string",
                        "description": "消息ID"
                    }
                },
                "required": ["msg_id"]
            }),
        }
    }

    async fn execute(&self, _args: serde_json::Value) -> XueliResult<String> {
        Ok("（当前不支持查看消息详情，请根据已有信息回复）".to_string())
    }
}

// ── tool_search 工具 ─────────────────────────────────────

/// 搜索工具 — 搜索可用扩展工具
struct ToolSearchTool;

#[async_trait]
impl Tool for ToolSearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "tool_search".to_string(),
            description: "搜索可用的额外工具。调用此工具可以发现之前未列出的扩展能力。".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "工具名或关键词"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, _args: serde_json::Value) -> XueliResult<String> {
        Ok("当前没有额外的扩展工具可用。".to_string())
    }
}

// ── 工具调用记录 ─────────────────────────────────────────

/// 工具调用记录
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub input_args: serde_json::Value,
    pub output_summary: String,
    pub duration_ms: f64,
    pub status: String,
    pub round_index: usize,
}

/// ReplyAgent 运行结果
#[derive(Debug, Clone)]
pub struct ReplyAgentResult {
    pub reply_text: String,
    pub reply_segments: Option<Vec<String>>,
    pub tool_call_records: Vec<ToolCallRecord>,
    pub source: String,
}

// ── ReplyAgent ───────────────────────────────────────────

/// 回复代理 — 带 tool-calling 循环的 AI 回复
///
/// 对应 Python 版 `xueli/src/handlers/reply/agent.py`
pub struct ReplyAgent<A: AIClient, L: PromptTemplateLoader> {
    ai_client: Arc<A>,
    memory_manager: Arc<MemoryManager>,
    person_fact_store: Arc<SqlitePersonFactStore>,
    prompt_builder: ReplyPromptBuilder<L>,
    /// 最大工具调用轮数
    max_rounds: usize,
    /// 结束工具名集合
    end_tool_names: HashSet<String>,
    /// 最大重试次数
    max_retries: usize,
}

impl<A: AIClient, L: PromptTemplateLoader> ReplyAgent<A, L> {
    pub fn new(
        ai_client: Arc<A>,
        memory_manager: Arc<MemoryManager>,
        person_fact_store: Arc<SqlitePersonFactStore>,
        prompt_builder: ReplyPromptBuilder<L>,
    ) -> Self {
        let mut end_tool_names = HashSet::new();
        end_tool_names.insert("reply".to_string());
        Self {
            ai_client,
            memory_manager,
            person_fact_store,
            prompt_builder,
            max_rounds: 3,
            end_tool_names,
            max_retries: 3,
        }
    }

    /// 构建当前会话可用工具列表
    fn build_tools(&self, user_id: &str) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(ReplyTool),
            Arc::new(QueryMemoryTool {
                memory_manager: Arc::clone(&self.memory_manager),
                user_id: user_id.to_string(),
            }),
            Arc::new(QueryPersonTool {
                person_fact_store: Arc::clone(&self.person_fact_store),
                user_id: user_id.to_string(),
            }),
            Arc::new(ViewMessageTool),
            Arc::new(ToolSearchTool),
        ]
    }

    /// 生成回复文本（带 tool-calling 循环）
    pub async fn generate_reply(
        &self,
        event: &InboundEvent,
        context: &ConversationContext,
        reply_reference: &str,
    ) -> XueliResult<ReplyAgentResult> {
        let user_message = context.user_message.clone();
        let sender_name = event
            .message
            .as_ref()
            .map(|m| m.sender_name.clone())
            .unwrap_or_else(|| "用户".to_string());
        let user_id = event
            .message
            .as_ref()
            .map(|m| m.sender_id.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let scope = if context.is_group {
            ChatScope::Group(
                event
                    .message
                    .as_ref()
                    .and_then(|m| m.scope.group_id().map(|s| s.to_string()))
                    .unwrap_or_default(),
            )
        } else {
            ChatScope::Private
        };

        // 构建 identity 文本
        let identity = format!("你的名字是「{sender_name}的好友」。你是一个友好的 AI 助手。");

        // 构建系统提示词
        let system_prompt = self
            .prompt_builder
            .build_system_prompt(
                &identity,
                &scope,
                reply_reference,
                &context.person_facts.clone().unwrap_or_default(),
                &context.memories.clone().unwrap_or_default(),
            )
            .await;

        let reference_hint = if reply_reference.is_empty() {
            String::new()
        } else {
            format!("\n\n【回复方向参考】\n{}", reply_reference)
        };

        let history_text = if context.recent_messages.is_empty() {
            "（无近期对话记录）".to_string()
        } else {
            context
                .recent_messages
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        };

        // 初始消息列表
        let mut messages: Vec<ChatMessage> = vec![
            ChatMessage::text("system", &system_prompt),
            ChatMessage::text(
                "user",
                &format!(
                    "近期对话记录：\n{}\n\n当前用户消息：{}{}",
                    history_text, user_message, reference_hint
                ),
            ),
        ];

        let tools = self.build_tools(&user_id);

        // 序列化工具定义为 API 格式
        let serialized_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                let def = t.definition();
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": def.name,
                        "description": def.description,
                        "parameters": def.parameters,
                    }
                })
            })
            .collect();

        // 带重试的工具调用循环
        let mut tool_call_records: Vec<ToolCallRecord> = Vec::new();
        let mut last_assistant_content = String::new();
        let mut retries = 0;

        'outer: loop {
            // 内部 tool-calling 循环
            for round_idx in 0..self.max_rounds {
                let mut extra_params = HashMap::new();
                if !serialized_tools.is_empty() {
                    extra_params.insert(
                        "tools".to_string(),
                        serde_json::Value::Array(serialized_tools.clone()),
                    );
                    extra_params.insert(
                        "tool_choice".to_string(),
                        serde_json::Value::String("auto".to_string()),
                    );
                }

                let request = ChatCompletionRequest {
                    model: "gpt-4o-mini".to_string(),
                    messages: messages.clone(),
                    temperature: Some(0.8),
                    max_tokens: Some(512),
                    stream: false,
                    extra_params,
                };

                let response = match self.ai_client.chat_completion(&request).await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("[{LOG_RETRY}] AI 调用失败 (第 {retries} 次重试): {e}");
                        retries += 1;
                        if retries >= self.max_retries {
                            return Err(format!("AI 调用失败，已重试 {retries} 次: {e}").into());
                        }
                        // 重试：退回外层循环
                        continue 'outer;
                    }
                };

                let content = response.content.clone();
                tracing::debug!(
                    "[{LOG_PROMPT_DIGEST}] assistant content: {}",
                    &content.chars().take(200).collect::<String>()
                );

                if !content.is_empty() {
                    last_assistant_content = content.clone();
                }

                let tool_calls = response.tool_calls.unwrap_or_default();

                if tool_calls.is_empty() {
                    // 没有工具调用，直接返回内容
                    return Ok(ReplyAgentResult {
                        reply_text: content,
                        reply_segments: None,
                        tool_call_records,
                        source: "agent".to_string(),
                    });
                }

                // 添加 assistant 消息（含 tool_calls）
                messages.push(ChatMessage::text("assistant", &content));

                let mut has_end_tool = false;
                let mut round_summaries: Vec<String> = Vec::new();

                for tc in &tool_calls {
                    let func = tc.function.clone();
                    let tool_name = func.name.clone();
                    let args_str = func.arguments.clone();
                    let args: serde_json::Value =
                        serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);

                    // 查找并执行工具
                    let tool_result = self
                        .execute_tool(&tools, &tool_name, args.clone())
                        .await
                        .unwrap_or_else(|e| format!("工具执行失败: {e}"));

                    // 添加工具结果消息
                    messages.push(ChatMessage {
                        role: "tool".to_string(),
                        content: MessageContent::Text(tool_result.clone()),
                        name: Some(tool_name.clone()),
                    });

                    tool_call_records.push(ToolCallRecord {
                        tool_name: tool_name.clone(),
                        input_args: args,
                        output_summary: tool_result.chars().take(200).collect(),
                        duration_ms: 0.0,
                        status: "ok".to_string(),
                        round_index: round_idx,
                    });

                    round_summaries.push(format!("[{tool_name}] → {tool_result}"));
                    if self.end_tool_names.contains(&tool_name) {
                        has_end_tool = true;
                    }
                }

                if has_end_tool {
                    // end tool 被调用
                    let reply_text = round_summaries.join("\n");
                    return Ok(ReplyAgentResult {
                        reply_text: reply_text.clone(),
                        reply_segments: Some(vec![reply_text]),
                        tool_call_records,
                        source: "agent_end_tool".to_string(),
                    });
                }
            }

            // 达到最大轮数
            break;
        }

        // 达到最大轮数，返回最后的内容
        Ok(ReplyAgentResult {
            reply_text: last_assistant_content,
            reply_segments: None,
            tool_call_records,
            source: "agent_max_rounds".to_string(),
        })
    }

    /// 执行工具
    async fn execute_tool(
        &self,
        tools: &[Arc<dyn Tool>],
        tool_name: &str,
        args: serde_json::Value,
    ) -> XueliResult<String> {
        for tool in tools {
            if tool.definition().name == tool_name {
                return tool.execute(args).await;
            }
        }
        Err(format!("未知工具: {tool_name}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reply_tool_definition() {
        let tool = ReplyTool;
        let def = tool.definition();
        assert_eq!(def.name, "reply");
        assert!(tool.is_end_tool());
    }

    #[test]
    fn test_tool_execution() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let tool = ReplyTool;
        let result = rt.block_on(tool.execute(serde_json::json!({"text": "你好世界"})));
        assert_eq!(result.unwrap(), "你好世界");
    }

    #[test]
    fn test_query_memory_tool_definition() {
        let tool = QueryMemoryTool {
            memory_manager: Arc::new(MemoryManager::new(
                Arc::new(crate::core::config::MemoryConfig {
                    db_path: ":memory:".to_string(),
                    extraction_min_messages: 5,
                    bm25_top_k: 10,
                    vector_top_k: 5,
                    dispute: Default::default(),
                }),
                Arc::new(
                    crate::memory::stores::memory_item::SqliteMemoryItemStore::new(
                        std::path::Path::new("/tmp/xueli_test"),
                    )
                    .unwrap(),
                ),
            )),
            user_id: "test_user".to_string(),
        };
        let def = tool.definition();
        assert_eq!(def.name, "query_memory");
    }

    #[test]
    fn test_query_person_tool_definition() {
        let tool = QueryPersonTool {
            person_fact_store: Arc::new(
                SqlitePersonFactStore::new(std::path::Path::new("/tmp/xueli_test")).unwrap(),
            ),
            user_id: "test_user".to_string(),
        };
        let def = tool.definition();
        assert_eq!(def.name, "query_person");
    }

    #[test]
    fn test_view_message_tool_definition() {
        let tool = ViewMessageTool;
        let def = tool.definition();
        assert_eq!(def.name, "view_message");
    }

    #[test]
    fn test_tool_search_definition() {
        let tool = ToolSearchTool;
        let def = tool.definition();
        assert_eq!(def.name, "tool_search");
    }
}
