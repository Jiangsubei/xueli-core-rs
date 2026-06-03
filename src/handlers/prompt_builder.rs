use std::collections::HashMap;

/// ReplyAgent 提示词构建器
pub struct ReplyPromptBuilder;

impl ReplyPromptBuilder {
    pub fn new() -> Self {
        Self
    }

    /// 构建系统提示词
    pub fn build_system_prompt(
        &self,
        _identity: &str,
        _style_guidance: &str,
        _memories: &[String],
    ) -> String {
        let mut parts = Vec::new();
        parts.push("你是一个友好的 AI 助手。".to_string());

        if !_memories.is_empty() {
            parts.push("\n相关记忆：".to_string());
            for mem in _memories {
                parts.push(format!("- {}", mem));
            }
        }

        parts.join("\n")
    }

    /// 构建用户消息内容
    pub fn build_user_message(
        &self,
        _sender_name: &str,
        _message_text: &str,
        _context: &HashMap<String, String>,
    ) -> String {
        _message_text.to_string()
    }
}

impl Default for ReplyPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}