//! 日志标签常量 — 统一管理日志中使用的标签字符串。
//!
//! 对应 Python 版 `xueli/src/handlers/shared/label_constants.py`

/// 完整提示词日志标签（便于排查 AI 输出异常）
pub const LOG_PROMPT_FULL: &str = "prompt_full";

/// 提示词摘要日志标签
pub const LOG_PROMPT_DIGEST: &str = "prompt_digest";

/// HTTP 访问日志标签
pub const LOG_ACCESS: &str = "access";

/// AI 重试日志标签
pub const LOG_RETRY: &str = "ai_retry";

/// 启动信息日志标签
pub const LOG_STARTUP_INFO: &str = "startup_info";

/// 会话类型标签
pub mod session_labels {
    pub const PRIVATE: &str = "私聊";
    pub const GROUP: &str = "群聊";
}

/// 发送者标签
pub mod sender_labels {
    pub const USER: &str = "用户";
    pub const ASSISTANT: &str = "助手";
    pub const DISPLAY_NAME_FALLBACK: &str = "助手";
}
