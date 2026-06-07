use crate::prelude::XueliResult;
use crate::traits::ai_client::{ChatMessage, ContentPart, MessageContent};
use crate::traits::tool_calling::ToolDefinition;

/// Token 计数器 — 基于 tiktoken 的 token 估算和预算管理。
///
/// 预算超限时只截断历史消息，不截断 system prompt 和当前用户输入。
pub struct TokenCounter {
    bpe: Option<tiktoken_rs::CoreBPE>,
}

impl TokenCounter {
    /// 使用 cl100k_base 编码创建计数器
    pub fn new_cl100k() -> XueliResult<Self> {
        match tiktoken_rs::cl100k_base() {
            Ok(bpe) => Ok(Self { bpe: Some(bpe) }),
            Err(e) => {
                tracing::warn!("tiktoken cl100k_base 加载失败: {}，使用零计数回退", e);
                Ok(Self { bpe: None })
            }
        }
    }

    /// 使用 o200k_base 编码创建计数器
    pub fn new_o200k() -> XueliResult<Self> {
        match tiktoken_rs::o200k_base() {
            Ok(bpe) => Ok(Self { bpe: Some(bpe) }),
            Err(e) => {
                tracing::warn!("tiktoken o200k_base 加载失败: {}，使用零计数回退", e);
                Ok(Self { bpe: None })
            }
        }
    }

    /// 计数器是否可用
    pub fn available(&self) -> bool {
        self.bpe.is_some()
    }

    /// 单文本 token 计数
    pub fn count(&self, text: &str) -> usize {
        match &self.bpe {
            Some(bpe) => bpe.encode_ordinary(text).len(),
            None => 0,
        }
    }

    /// 计算单条消息的 token 开销（含 role overhead）
    fn count_single_message(&self, msg: &ChatMessage) -> usize {
        let mut total = 0;
        total += self.count("role: ");
        total += self.count(&msg.role);
        total += 1;
        total += self.count_content(&msg.content);
        if let Some(ref name) = msg.name {
            total += self.count("name: ");
            total += self.count(name);
            total += 1;
        }
        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                total += self.count(&tc.function.name);
                total += self.count(&tc.function.arguments);
                total += 11;
            }
        }
        if let Some(ref tool_call_id) = msg.tool_call_id {
            total += self.count(tool_call_id);
        }
        total
    }

    /// 计算消息内容的 token 开销（支持纯文本与多模态）
    fn count_content(&self, content: &MessageContent) -> usize {
        match content {
            MessageContent::Text(s) => self.count(s),
            MessageContent::Multimodal(parts) => {
                let mut total = 0;
                for part in parts {
                    match part {
                        ContentPart::Text { text } => {
                            total += self.count(text);
                        }
                        ContentPart::ImageUrl { image_url } => {
                            if image_url.url.starts_with("data:") {
                                total += 85;
                            } else {
                                total += self.count(&image_url.url);
                            }
                        }
                    }
                    total += 2;
                }
                total
            }
        }
    }

    /// 计算多条消息的 token 数（含消息级 + 请求级 overhead）
    pub fn count_messages(&self, messages: &[ChatMessage]) -> usize {
        let mut total: usize = 0;
        for msg in messages {
            total += self.count_single_message(msg);
        }
        total + 3 // 请求级固定 overhead
    }

    /// 计算 tools 定义（函数 schema）的 token 开销
    pub fn count_tool_definitions(&self, tools: &[ToolDefinition]) -> usize {
        let mut total = 0;
        for tool in tools {
            total += self.count(&tool.name);
            total += self.count(&tool.description);
            let params_str = serde_json::to_string(&tool.parameters_schema).unwrap_or_default();
            total += self.count(&params_str);
            total += 8;
        }
        total
    }

    /// 统一的消息裁剪器：根据 token 预算从历史消息中保留最新的若干条。
    ///
    /// 裁剪策略：
    /// 1. system 消息和当前用户消息是硬性消耗，不截断
    /// 2. tools 定义占用的 token 同样不可截断
    /// 3. 历史消息按时间升序，从最新端向最旧端贪心填充
    /// 4. 若 budget 不足以容纳硬性消耗，返回 0 条历史
    ///
    /// 返回：裁剪后的完整消息列表 + 被丢弃的历史消息数
    pub fn trim_messages_to_budget(
        &self,
        system_message: &ChatMessage,
        current_message: &ChatMessage,
        history_messages: &[ChatMessage],
        budget: usize,
        tool_reserve: usize,
        tools: Option<&[ToolDefinition]>,
        tag: &str,
    ) -> (Vec<ChatMessage>, usize) {
        let tool_def_cost = tools.map(|t| self.count_tool_definitions(t)).unwrap_or(0);

        let hard_cost = self.count_messages(&[system_message.clone()])
            + self.count_messages(&[current_message.clone()])
            + tool_reserve
            + tool_def_cost;

        let history_budget = budget.saturating_sub(hard_cost);

        if history_budget == 0 {
            tracing::warn!(
                "[CTX BUDGET {}] 硬性消耗已超预算！hard={} budget={}",
                tag,
                hard_cost,
                budget
            );
            return (
                vec![system_message.clone(), current_message.clone()],
                history_messages.len(),
            );
        }

        let mut accumulated = 0;
        let mut selected: Vec<ChatMessage> = Vec::new();

        for hist_msg in history_messages.iter().rev() {
            let msg_tokens = self.count_messages(&[hist_msg.clone()]);
            if accumulated + msg_tokens <= history_budget {
                accumulated += msg_tokens;
                selected.insert(0, hist_msg.clone());
            } else {
                break;
            }
        }

        let kept_count = selected.len();
        let skipped = history_messages.len() - kept_count;

        let mut result = vec![system_message.clone()];
        result.extend(selected);
        result.push(current_message.clone());

        tracing::info!(
            "[CTX BUDGET {}] budget={} hard={} history_used={} kept={}/{} skipped={}",
            tag,
            budget,
            hard_cost,
            accumulated,
            kept_count,
            history_messages.len(),
            skipped,
        );

        (result, skipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_text() {
        let counter = TokenCounter::new_cl100k().unwrap();
        let n = counter.count("hello world");
        assert!(n > 0);
    }

    #[test]
    fn test_count_messages_basic() {
        let counter = TokenCounter::new_cl100k().unwrap();
        let msgs = vec![
            ChatMessage::text("system", "你是一个助手"),
            ChatMessage::text("user", "你好"),
        ];
        let n = counter.count_messages(&msgs);
        assert!(n > 5);
    }

    #[test]
    fn test_count_tool_calls() {
        let counter = TokenCounter::new_cl100k().unwrap();
        let msg = ChatMessage::assistant_with_tool_calls(
            "",
            vec![crate::traits::ai_client::ToolCall {
                id: "call_1".to_string(),
                call_type: "function".to_string(),
                function: crate::traits::ai_client::FunctionCall {
                    name: "search_memory".to_string(),
                    arguments: r#"{"query":"天气"}"#.to_string(),
                },
            }],
        );
        let n = counter.count_messages(&[msg]);
        assert!(n > 20); // role overhead + name tokens + args tokens + 11 per tool_call
    }

    #[test]
    fn test_count_tool_call_id() {
        let counter = TokenCounter::new_cl100k().unwrap();
        let msg = ChatMessage::tool_result("call_abc123", "结果内容");
        let n = counter.count_messages(&[msg]);
        let msg_no_id = ChatMessage::text("tool", "结果内容");
        let n_no_id = counter.count_messages(&[msg_no_id]);
        assert!(n > n_no_id);
    }

    #[test]
    fn test_count_multimodal_content() {
        let counter = TokenCounter::new_cl100k().unwrap();
        let msg = ChatMessage::multimodal(
            "user",
            "描述这张图片",
            &["iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==".to_string()],
            "image/png",
        );
        let n = counter.count_messages(&[msg]);
        // text tokens + 85 for image data URI + 2 part overhead × 2 parts + message overhead
        assert!(n > 90);
    }

    #[test]
    fn test_trim_to_budget_enough() {
        let counter = TokenCounter::new_cl100k().unwrap();
        let sys = ChatMessage::text("system", "你是助手");
        let cur = ChatMessage::text("user", "今天天气怎么样");
        let hist: Vec<ChatMessage> = vec![
            ChatMessage::text("user", "你好"),
            ChatMessage::text("assistant", "你好！"),
        ];

        let (result, skipped) =
            counter.trim_messages_to_budget(&sys, &cur, &hist, 1000, 0, None, "test");
        assert_eq!(skipped, 0);
        assert!(result.len() >= 3);
    }

    #[test]
    fn test_trim_to_budget_tight() {
        let counter = TokenCounter::new_cl100k().unwrap();
        let sys = ChatMessage::text("system", "你是助手");
        let cur = ChatMessage::text("user", "hello");

        let hist: Vec<ChatMessage> = (0..50)
            .map(|i| ChatMessage::text("user", &format!("msg {}", i)))
            .collect();
        let total = hist.len();

        let (result, skipped) =
            counter.trim_messages_to_budget(&sys, &cur, &hist, 100, 0, None, "test");
        assert!(skipped > 0);
        assert!(result.len() < total + 2);
    }
}
