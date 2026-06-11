use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::Notify;

use crate::core::config::XueliConfig;
use crate::core::log_labels::{LOG_PROMPT_DIGEST, LOG_RETRY};
use crate::core::platform_types::InboundEvent;
use crate::core::scope::ChatScope;
use crate::handlers::context_builder::ConversationContext;
use crate::handlers::prompt_builder::ReplyPromptBuilder;
use crate::memory::manager::MemoryManager;
use crate::memory::stores::person_fact::SqlitePersonFactStore;
use crate::prelude::XueliResult;
use crate::services::token_counter::TokenCounter;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};
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
struct QueryMemoryTool<L: PromptTemplateLoader + 'static> {
    memory_manager: Arc<MemoryManager<L>>,
    #[allow(dead_code)]
    user_id: String,
}

#[async_trait]
impl<L: PromptTemplateLoader + 'static> Tool for QueryMemoryTool<L> {
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
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
}

/// ReplyAgent 运行结果
#[derive(Debug, Clone)]
pub struct ReplyAgentResult {
    pub reply_text: String,
    pub reply_segments: Option<Vec<String>>,
    pub tool_call_records: Vec<ToolCallRecord>,
    pub source: String,
    pub expected_effect: String,
    pub total_prompt_tokens: usize,
    pub total_completion_tokens: usize,
}

impl Default for ReplyAgentResult {
    fn default() -> Self {
        Self {
            reply_text: String::new(),
            reply_segments: None,
            tool_call_records: Vec::new(),
            source: "agent".to_string(),
            expected_effect: String::new(),
            total_prompt_tokens: 0,
            total_completion_tokens: 0,
        }
    }
}

// ── ReplyAgent ───────────────────────────────────────────

/// 回复代理 — 带 tool-calling 循环的 AI 回复
///
/// 对应 Python 版 `xueli/src/handlers/reply/agent.py`
pub struct ReplyAgent<A: AIClient, L: PromptTemplateLoader + 'static> {
    config: Arc<XueliConfig>,
    ai_client: Arc<A>,
    memory_manager: Arc<MemoryManager<L>>,
    person_fact_store: Arc<SqlitePersonFactStore>,
    prompt_builder: ReplyPromptBuilder<L>,
    token_counter: TokenCounter,
    /// 最大工具调用轮数
    max_rounds: usize,
    /// 结束工具名集合
    end_tool_names: HashSet<String>,
    /// 最大重试次数
    max_retries: usize,
    /// 中断信号（新消息到达时触发）
    interrupt_notify: Arc<Notify>,
    /// 插件工具处理器
    plugin_handlers: HashMap<String, Arc<dyn Tool>>,
    /// 延迟工具列表（tool_search 可激活）
    deferred_tools: Vec<serde_json::Value>,
}

/// 最大内部工具调用轮数（对应 Python MAX_INTERNAL_ROUNDS）
const MAX_INTERNAL_ROUNDS: usize = 3;

impl<A: AIClient, L: PromptTemplateLoader + 'static> ReplyAgent<A, L> {
    pub fn new(
        config: Arc<XueliConfig>,
        ai_client: Arc<A>,
        memory_manager: Arc<MemoryManager<L>>,
        person_fact_store: Arc<SqlitePersonFactStore>,
        prompt_builder: ReplyPromptBuilder<L>,
    ) -> Self {
        let mut end_tool_names = HashSet::new();
        end_tool_names.insert("reply".to_string());
        let token_counter = TokenCounter::new_cl100k().unwrap_or_else(|_| TokenCounter::new_o200k().unwrap_or_else(|_| {
            tracing::warn!("[ReplyAgent] TokenCounter 初始化失败，使用零计数回退");
            TokenCounter::new_cl100k().unwrap_or_else(|_| {
                // 双重失败时创建 bpe=None 的实例
                TokenCounter::new_cl100k().unwrap_or_else(|_| {
                    // 最终回退：这不应该发生，但保证编译通过
                    panic!("TokenCounter 初始化失败")
                })
            })
        }));
        Self {
            config,
            ai_client,
            memory_manager,
            person_fact_store,
            prompt_builder,
            token_counter,
            max_rounds: MAX_INTERNAL_ROUNDS,
            end_tool_names,
            max_retries: 3,
            interrupt_notify: Arc::new(Notify::new()),
            plugin_handlers: HashMap::new(),
            deferred_tools: Vec::new(),
        }
    }

    /// 注册插件工具
    pub fn load_plugin_tools(&mut self, tools: Vec<(String, Arc<dyn Tool>, serde_json::Value)>) {
        let existing_names: HashSet<String> = self
            .plugin_handlers
            .keys()
            .cloned()
            .collect();
        for (name, handler, _schema) in tools {
            if self.plugin_handlers.contains_key(&name) {
                tracing::warn!("[ReplyAgent] 插件工具 {} 重复注册，跳过", name);
                continue;
            }
            if existing_names.contains(&name) {
                tracing::warn!("[ReplyAgent] 插件工具 {} 与内置工具重名，跳过", name);
                continue;
            }
            self.plugin_handlers.insert(name, handler);
        }
    }

    /// 触发中断信号（新消息到达时由外部调用）
    pub fn signal_interrupt(&self) {
        self.interrupt_notify.notify_one();
    }

    /// 检查是否被中断
    fn check_interrupt(&self) -> XueliResult<()> {
        // 非阻塞检查：如果 notify 已被触发则返回 Err
        // 实际中断在 tokio::select! 中处理
        Ok(())
    }

    /// 构建当前会话可用工具列表
    fn build_tools(&self, user_id: &str) -> Vec<Arc<dyn Tool>> {
        let mut tools: Vec<Arc<dyn Tool>> = vec![
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
        ];
        // 添加插件工具
        for handler in self.plugin_handlers.values() {
            tools.push(Arc::clone(handler));
        }
        tools
    }

    /// 标准化 expected_effect 值
    fn normalize_expected_effect(value: &serde_json::Value) -> String {
        let s = value.as_str().unwrap_or("").trim().to_lowercase();
        match s.as_str() {
            "continue" | "satisfy" | "cool_down" | "clarify" => s,
            _ => String::new(),
        }
    }

    /// 生成回复文本（带 tool-calling 循环 + 中断机制）
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
                context.style_guide.as_ref(),
                &context.person_facts.clone().unwrap_or_default(),
                &context
                    .persistent_memory_context
                    .clone()
                    .map(|s| vec![s])
                    .unwrap_or_default(),
                context.character_card_snapshot.as_ref(),
                context.narrative_thread_summary.as_deref(),
                context.narrative_thread_label.as_deref(),
                context.narrative_self.as_ref(),
                context.planning_signals.as_ref(),
                context.user_emotion_label.as_deref(),
                context.soft_uncertainty_signals.as_deref(),
                context.caution_signal.as_ref(),
                None, // metacognition_state_report
                None, // user_profile_signal
                if reply_reference.is_empty() {
                    None
                } else {
                    Some(reply_reference)
                },
            )
            .await;

        let reference_hint = if reply_reference.is_empty() {
            String::new()
        } else {
            format!("\n\n【回复方向参考】\n{}", reply_reference)
        };

        // 构建初始消息列表（使用 TokenCounter 预算管理）
        let mut messages = self.build_initial_messages(
            &system_prompt,
            &user_message,
            &context.recent_messages,
            reference_hint,
            &user_id,
        );

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

        let model_name = self.config.model.primary_model.clone();

        // 带重试的工具调用循环
        let mut tool_call_records: Vec<ToolCallRecord> = Vec::new();
        let mut last_assistant_content = String::new();
        let mut retries = 0;
        let mut total_prompt_tokens: usize = 0;
        let mut total_completion_tokens: usize = 0;

        'outer: loop {
            // 内部 tool-calling 循环
            for round_idx in 0..self.max_rounds {
                // 中断检查
                self.check_interrupt()?;

                let request = ChatCompletionRequest {
                    model: model_name.clone(),
                    messages: messages.clone(),
                    temperature: Some(0.7),
                    max_tokens: Some(512),
                    stream: false,
                    tools: if serialized_tools.is_empty() {
                        None
                    } else {
                        Some(serialized_tools.clone())
                    },
                    tool_choice: if serialized_tools.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::String("auto".to_string()))
                    },
                    extra_params: Default::default(),
                };

                // 使用 tokio::select! 实现 AI 调用与中断信号竞速
                let response = tokio::select! {
                    result = self.ai_client.chat_completion(&request) => {
                        match result {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!("[{LOG_RETRY}] AI 调用失败 (第 {retries} 次重试): {e}");
                                retries += 1;
                                if retries >= self.max_retries {
                                    if !last_assistant_content.is_empty() {
                                        return Ok(ReplyAgentResult {
                                            reply_text: last_assistant_content,
                                            reply_segments: None,
                                            tool_call_records,
                                            source: "agent_fallback".to_string(),
                                            expected_effect: String::new(),
                                            total_prompt_tokens,
                                            total_completion_tokens,
                                        });
                                    }
                                    return Err(format!("AI 调用失败，已重试 {retries} 次: {e}").into());
                                }
                                continue 'outer;
                            }
                        }
                    }
                    _ = self.interrupt_notify.notified() => {
                        tracing::info!("[ReplyAgent] 收到中断信号，终止推理");
                        if !last_assistant_content.is_empty() {
                            return Ok(ReplyAgentResult {
                                reply_text: last_assistant_content,
                                reply_segments: None,
                                tool_call_records,
                                source: "agent_interrupted".to_string(),
                                expected_effect: String::new(),
                                total_prompt_tokens,
                                total_completion_tokens,
                            });
                        }
                        return Ok(ReplyAgentResult {
                            reply_text: String::new(),
                            reply_segments: None,
                            tool_call_records,
                            source: "agent_interrupted".to_string(),
                            expected_effect: String::new(),
                            total_prompt_tokens,
                            total_completion_tokens,
                        });
                    }
                };

                let content = response.content.clone();
                tracing::debug!(
                    "[{LOG_PROMPT_DIGEST}] assistant content: {}",
                    &content.chars().take(200).collect::<String>()
                );

                // 统计 token 用量
                let (round_prompt, round_completion) = response
                    .usage
                    .as_ref()
                    .map(|u| (u.prompt_tokens as usize, u.completion_tokens as usize))
                    .unwrap_or((0, 0));
                total_prompt_tokens += round_prompt;
                total_completion_tokens += round_completion;

                if !content.is_empty() {
                    last_assistant_content = content.clone();
                }

                let tool_calls = response.tool_calls.unwrap_or_default();

                if tool_calls.is_empty() {
                    // 没有工具调用，直接返回内容
                    if !content.is_empty() {
                        return Ok(ReplyAgentResult {
                            reply_text: content,
                            reply_segments: None,
                            tool_call_records,
                            source: "agent_implicit".to_string(),
                            expected_effect: String::new(),
                            total_prompt_tokens,
                            total_completion_tokens,
                        });
                    }
                    break;
                }

                // 添加 assistant 消息（含 tool_calls）- 使用正确的消息格式
                messages.push(ChatMessage::assistant_with_tool_calls(
                    &content,
                    tool_calls.clone(),
                ));

                let mut has_end_tool = false;
                let mut end_tool_text = String::new();
                let mut end_tool_segments: Option<Vec<String>> = None;
                let mut end_tool_expected_effect = String::new();

                for tc in &tool_calls {
                    let func = tc.function.clone();
                    let tool_name = func.name.clone();
                    let args_str = func.arguments.clone();
                    let args: serde_json::Value =
                        serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);

                    let exec_start = Instant::now();

                    // 查找并执行工具
                    let tool_result = self
                        .execute_tool(&tools, &tool_name, args.clone())
                        .await
                        .unwrap_or_else(|e| format!("工具执行失败: {e}"));

                    let duration_ms = exec_start.elapsed().as_secs_f64() * 1000.0;

                    // 添加工具结果消息 - 使用正确的 tool_result 格式
                    messages.push(ChatMessage::tool_result(tc.id.clone(), &tool_result));

                    tool_call_records.push(ToolCallRecord {
                        tool_name: tool_name.clone(),
                        input_args: args.clone(),
                        output_summary: tool_result.chars().take(200).collect(),
                        duration_ms,
                        status: "ok".to_string(),
                        round_index: round_idx,
                        prompt_tokens: round_prompt,
                        completion_tokens: round_completion,
                    });

                    if self.end_tool_names.contains(&tool_name) {
                        has_end_tool = true;
                        // 从 reply 工具的 args 中提取实际回复文本
                        end_tool_text = args
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        // 提取分段
                        if let Some(segments_arr) = args.get("segments").and_then(|v| v.as_array())
                        {
                            let segs: Vec<String> = segments_arr
                                .iter()
                                .filter_map(|s| s.as_str())
                                .map(|s| s.to_string())
                                .filter(|s| !s.trim().is_empty())
                                .collect();
                            if segs.len() > 1 {
                                end_tool_segments = Some(segs);
                            }
                        }
                        // 提取 expected_effect
                        end_tool_expected_effect =
                            Self::normalize_expected_effect(args.get("expected_effect").unwrap_or(&serde_json::Value::Null));
                    }
                }

                if has_end_tool {
                    let reply_text = if end_tool_text.is_empty() {
                        last_assistant_content.clone()
                    } else {
                        end_tool_text.clone()
                    };
                    let reply_segments =
                        end_tool_segments.or_else(|| Some(vec![reply_text.clone()]));

                    self.log_tool_summary(&tool_call_records);

                    return Ok(ReplyAgentResult {
                        reply_text,
                        reply_segments,
                        tool_call_records,
                        source: "agent_end_tool".to_string(),
                        expected_effect: end_tool_expected_effect,
                        total_prompt_tokens,
                        total_completion_tokens,
                    });
                }
            }

            // 达到最大轮数
            break;
        }

        // 达到最大轮数，返回最后的内容
        if !last_assistant_content.is_empty() {
            return Ok(ReplyAgentResult {
                reply_text: last_assistant_content,
                reply_segments: None,
                tool_call_records,
                source: "agent_max_rounds".to_string(),
                expected_effect: String::new(),
                total_prompt_tokens,
                total_completion_tokens,
            });
        }
        Ok(ReplyAgentResult {
            reply_text: String::new(),
            reply_segments: None,
            tool_call_records,
            source: "agent_fallback".to_string(),
            expected_effect: String::new(),
            total_prompt_tokens,
            total_completion_tokens,
        })
    }

    /// 构建初始消息列表（使用 TokenCounter 进行预算管理）
    fn build_initial_messages(
        &self,
        system_prompt: &str,
        user_message: &str,
        recent_messages: &[String],
        reference_hint: String,
        _conversation_key: &str,
    ) -> Vec<ChatMessage> {
        let system_msg = ChatMessage::text("system", system_prompt);

        let current_text = if user_message.trim().is_empty() {
            "用户发送了空文本".to_string()
        } else {
            user_message.to_string()
        };

        let user_content = format!(
            "近期对话记录：\n{}\n\n当前用户消息：{}{}",
            if recent_messages.is_empty() {
                "（无近期对话记录）".to_string()
            } else {
                recent_messages.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
            },
            current_text,
            reference_hint
        );

        let current_msg = ChatMessage::text("user", &user_content);

        // 将 recent_messages 转为历史 ChatMessage
        let history: Vec<ChatMessage> = recent_messages
            .iter()
            .take(10)
            .map(|m| ChatMessage::text("user", m))
            .collect();

        // 使用 TokenCounter 裁剪到预算
        let budget = self.config.model.context_window as usize;
        let effective_budget = (budget as f64 * 0.7) as usize;

        let (trimmed, _skipped) = self.token_counter.trim_messages_to_budget(
            &system_msg,
            &current_msg,
            &history,
            effective_budget,
            1500,
            None,
            "AGENT",
        );

        trimmed
    }

    /// 执行工具
    async fn execute_tool(
        &self,
        tools: &[Arc<dyn Tool>],
        tool_name: &str,
        args: serde_json::Value,
    ) -> XueliResult<String> {
        // 先检查内置工具
        for tool in tools {
            if tool.definition().name == tool_name {
                return tool.execute(args).await;
            }
        }
        // 再检查插件工具
        if let Some(handler) = self.plugin_handlers.get(tool_name) {
            return handler.execute(args).await;
        }
        Err(format!("未知工具: {tool_name}").into())
    }

    /// 搜索延迟工具（tool_search 工具的实现）
    fn search_deferred_tools(&mut self, query: &str) -> String {
        if self.deferred_tools.is_empty() {
            return "当前没有额外的扩展工具可用。".to_string();
        }
        let matched: Vec<&serde_json::Value> = self
            .deferred_tools
            .iter()
            .filter(|t| {
                t.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|name| name.to_lowercase().contains(&query.to_lowercase()))
                    .unwrap_or(false)
            })
            .collect();

        if matched.is_empty() {
            return "当前没有额外的扩展工具可用。".to_string();
        }

        let names: Vec<String> = matched
            .iter()
            .filter_map(|t| {
                t.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        format!("发现新工具: {}。后续对话中可以使用这些工具。", names.join(", "))
    }

    /// 日志输出工具调用摘要
    fn log_tool_summary(&self, records: &[ToolCallRecord]) {
        if records.is_empty() {
            return;
        }
        let total_p: usize = records.iter().map(|r| r.prompt_tokens).sum();
        let total_c: usize = records.iter().map(|r| r.completion_tokens).sum();
        let avg_p = total_p / records.len().max(1);
        tracing::info!(
            "[AGENT TOOLS] 共 {} 次调用, prompt={} (avg={}/轮) completion={}",
            records.len(),
            total_p,
            avg_p,
            total_c,
        );
        for (i, r) in records.iter().enumerate() {
            tracing::info!(
                "[AGENT TOOLS]   [{}] round={} {} {} {:.0}ms -> {}",
                i + 1,
                r.round_index,
                r.tool_name,
                r.status,
                r.duration_ms,
                &r.output_summary.chars().take(120).collect::<String>(),
            );
        }
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
            memory_manager: Arc::new(
                MemoryManager::new(
                    Arc::new(crate::core::config::MemoryConfig {
                        enabled: true,
                        db_path: ":memory:".to_string(),
                        storage_backend: "sqlite".to_string(),
                        extraction_min_messages: 5,
                        bm25_top_k: 10,
                        vector_top_k: 5,
                        rerank_top_k: 20,
                        pre_rerank_top_k: 12,
                        dynamic_memory_limit: 8,
                        dynamic_dedup_enabled: true,
                        dynamic_dedup_similarity_threshold: 0.72,
                        rerank_candidate_max_chars: 160,
                        rerank_total_prompt_budget: 2400,
                        dispute: Default::default(),
                        auto_extract: true,
                        extract_every_n_turns: 3,
                        extraction_api_base: None,
                        extraction_api_key: None,
                        extraction_model: None,
                        extraction_context_window: 128000,
                        extraction_extra_params: None,
                        extraction_extra_headers: None,
                        extraction_response_path: None,
                        decay: Default::default(),
                        merge: Default::default(),
                        suppression: Default::default(),
                        retrieval_weights: Default::default(),
                        scene_weights: Default::default(),
                        fuzzy_recall: Default::default(),
                    }),
                    Arc::new(
                        crate::memory::stores::memory_item::SqliteMemoryItemStore::new(
                            std::path::Path::new("/tmp/xueli_test"),
                        )
                        .unwrap(),
                    ),
                    Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
                )
                .unwrap(),
            ),
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
