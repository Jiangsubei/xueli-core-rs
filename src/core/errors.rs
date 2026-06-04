use thiserror::Error;

/// xueli-core 统一错误类型
#[derive(Error, Debug)]
pub enum XueliError {
    /// 配置错误
    #[error("配置错误: {0}")]
    Config(String),

    /// AI 服务错误
    #[error("AI 服务错误: {0}")]
    AIService(String),

    /// 记忆系统错误
    #[error("记忆系统错误: {0}")]
    Memory(String),

    /// 数据库错误
    #[error("数据库错误: {0}")]
    Database(String),

    /// 序列化/反序列化错误
    #[error("序列化错误: {0}")]
    Serialization(String),

    /// Token 计数错误
    #[error("Token 计数错误: {0}")]
    TokenCount(String),

    /// 模板加载错误
    #[error("模板错误: {0}")]
    Template(String),

    /// 工具调用错误
    #[error("工具调用错误: {0}")]
    ToolCall(String),

    /// 平台适配错误
    #[error("平台适配错误: {0}")]
    Platform(String),

    /// 会话错误
    #[error("会话错误: {0}")]
    Session(String),

    /// 管线程错误
    #[error("管线错误: {0}")]
    Pipeline(String),

    /// 一般错误
    #[error("内部错误: {0}")]
    Internal(String),
}

/// 便捷类型别名
pub type XueliResult<T> = Result<T, XueliError>;

// ── From 转换实现 ────────────────────────────────────────

impl From<String> for XueliError {
    fn from(e: String) -> Self {
        XueliError::Internal(e)
    }
}

impl From<&str> for XueliError {
    fn from(e: &str) -> Self {
        XueliError::Internal(e.to_string())
    }
}

impl From<rusqlite::Error> for XueliError {
    fn from(e: rusqlite::Error) -> Self {
        XueliError::Database(e.to_string())
    }
}

impl From<serde_json::Error> for XueliError {
    fn from(e: serde_json::Error) -> Self {
        XueliError::Serialization(e.to_string())
    }
}

impl From<reqwest::Error> for XueliError {
    fn from(e: reqwest::Error) -> Self {
        XueliError::AIService(e.to_string())
    }
}

impl From<std::io::Error> for XueliError {
    fn from(e: std::io::Error) -> Self {
        XueliError::Internal(e.to_string())
    }
}
