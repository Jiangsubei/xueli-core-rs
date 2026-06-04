use crate::prelude::XueliResult;

/// Token 计数器 — 估算文本 token 用量
pub struct TokenCounter {
    bpe: tiktoken_rs::CoreBPE,
}

impl TokenCounter {
    /// 使用 cl100k_base 编码创建计数器
    pub fn new_cl100k() -> XueliResult<Self> {
        let bpe =
            tiktoken_rs::cl100k_base().map_err(|e| format!("加载 tiktoken BPE 失败: {}", e))?;
        Ok(Self { bpe })
    }

    /// 使用 o200k_base 编码创建计数器
    pub fn new_o200k() -> XueliResult<Self> {
        let bpe =
            tiktoken_rs::o200k_base().map_err(|e| format!("加载 tiktoken BPE 失败: {}", e))?;
        Ok(Self { bpe })
    }

    /// 计算文本 token 数
    pub fn count(&self, text: &str) -> usize {
        self.bpe.encode_ordinary(text).len()
    }

    /// 计算多条消息的 token 数
    pub fn count_messages(&self, messages: &[crate::traits::ai_client::ChatMessage]) -> usize {
        messages.iter().map(|m| self.count(&m.content.text())).sum()
    }
}
