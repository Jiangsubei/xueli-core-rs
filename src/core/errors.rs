use std::fmt;

use thiserror::Error;

/// xueli-core 统一错误类型
#[derive(Error, Debug, Clone)]
pub enum XueliError {
    /// 配置错误
    #[error("配置错误: {0}")]
    Config(String),

    /// AI 服务错误
    #[error("AI 服务错误: {0}")]
    AIService(AIServiceError),

    /// 视觉服务错误
    #[error("视觉服务错误: {0}")]
    VisionService(String),

    /// 记忆系统错误
    #[error("记忆系统错误: {0}")]
    Memory(MemoryError),

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
    Template(TemplateError),

    /// 工具调用错误
    #[error("工具调用错误: {0}")]
    ToolCall(String),

    /// 平台适配错误
    #[error("平台适配错误: {0}")]
    Platform(PlatformError),

    /// 会话错误
    #[error("会话错误: {0}")]
    Session(String),

    /// 管线执行错误
    #[error("管线错误: {category} - {message}")]
    Pipeline {
        category: PipelineErrorCategory,
        message: String,
    },

    /// 请求被中断（新消息到达等）
    #[error("请求被中断")]
    ReqAbort,

    /// 一般内部错误
    #[error("内部错误: {0}")]
    Internal(String),
}

/// AI 服务错误子类型
#[derive(Error, Debug, Clone)]
pub enum AIServiceError {
    #[error("模型请求失败: {0}")]
    ModelRequest(String),
    #[error("模型响应解析失败: {0}")]
    ModelParse(String),
    #[error("图像处理失败: {0}")]
    ImageProcessing(String),
    #[error("API 错误: {0}")]
    ApiError(String),
    #[error("超时")]
    Timeout,
    #[error("速率限制")]
    RateLimited,
}

/// 记忆系统错误子类型
#[derive(Error, Debug, Clone)]
pub enum MemoryError {
    #[error("记忆提取失败: {0}")]
    Extraction(String),
    #[error("记忆检索失败: {0}")]
    Retrieval(String),
    #[error("记忆存储失败: {0}")]
    Storage(String),
    #[error("记忆合并失败: {0}")]
    Merge(String),
    #[error("记忆冲突: {0}")]
    Dispute(String),
    #[error("记忆衰减计算失败: {0}")]
    Decay(String),
}

/// 模板错误子类型
#[derive(Error, Debug, Clone)]
pub enum TemplateError {
    #[error("模板未找到: {0}")]
    NotFound(String),
    #[error("模板渲染失败: {0}")]
    Render(String),
    #[error("模板加载失败: {0}")]
    Load(String),
}

/// 平台适配错误子类型
#[derive(Error, Debug, Clone)]
pub enum PlatformError {
    #[error("连接失败: {0}")]
    Connection(String),
    #[error("消息发送失败: {0}")]
    Send(String),
    #[error("消息接收失败: {0}")]
    Receive(String),
    #[error("平台认证失败: {0}")]
    Auth(String),
    #[error("WebSocket 错误: {0}")]
    WebSocket(String),
}

/// 管线错误分类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineErrorCategory {
    Configuration,
    ModelRequest,
    ModelParse,
    ImageProcessing,
    MemoryOperation,
    Send,
    PipelineExecution,
    Platform,
}

impl fmt::Display for PipelineErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineErrorCategory::Configuration => write!(f, "configuration_error"),
            PipelineErrorCategory::ModelRequest => write!(f, "model_request_error"),
            PipelineErrorCategory::ModelParse => write!(f, "model_parse_error"),
            PipelineErrorCategory::ImageProcessing => write!(f, "image_processing_error"),
            PipelineErrorCategory::MemoryOperation => write!(f, "memory_error"),
            PipelineErrorCategory::Send => write!(f, "send_error"),
            PipelineErrorCategory::PipelineExecution => write!(f, "pipeline_error"),
            PipelineErrorCategory::Platform => write!(f, "platform_error"),
        }
    }
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
        if e.is_timeout() {
            XueliError::AIService(AIServiceError::Timeout)
        } else if e.status().map_or(false, |s| s == 429) {
            XueliError::AIService(AIServiceError::RateLimited)
        } else {
            XueliError::AIService(AIServiceError::ModelRequest(e.to_string()))
        }
    }
}

impl From<std::io::Error> for XueliError {
    fn from(e: std::io::Error) -> Self {
        XueliError::Internal(e.to_string())
    }
}

impl From<toml::de::Error> for XueliError {
    fn from(e: toml::de::Error) -> Self {
        XueliError::Config(e.to_string())
    }
}

impl From<config::ConfigError> for XueliError {
    fn from(e: config::ConfigError) -> Self {
        XueliError::Config(e.to_string())
    }
}

// ── 便捷构造函数 ─────────────────────────────────────────

impl XueliError {
    /// 创建配置错误
    pub fn config(msg: impl Into<String>) -> Self {
        XueliError::Config(msg.into())
    }

    /// 创建 AI 服务请求错误
    pub fn model_request(msg: impl Into<String>) -> Self {
        XueliError::AIService(AIServiceError::ModelRequest(msg.into()))
    }

    /// 创建 AI 服务解析错误
    pub fn model_parse(msg: impl Into<String>) -> Self {
        XueliError::AIService(AIServiceError::ModelParse(msg.into()))
    }

    /// 创建图像处理错误
    pub fn image_processing(msg: impl Into<String>) -> Self {
        XueliError::AIService(AIServiceError::ImageProcessing(msg.into()))
    }

    /// 创建记忆操作错误
    pub fn memory(msg: impl Into<String>) -> Self {
        XueliError::Memory(MemoryError::Storage(msg.into()))
    }

    /// 创建发送错误
    pub fn send(msg: impl Into<String>) -> Self {
        XueliError::Platform(PlatformError::Send(msg.into()))
    }

    /// 创建管线执行错误
    pub fn pipeline(category: PipelineErrorCategory, msg: impl Into<String>) -> Self {
        XueliError::Pipeline {
            category,
            message: msg.into(),
        }
    }

    /// 创建平台适配错误
    pub fn platform(msg: impl Into<String>) -> Self {
        XueliError::Platform(PlatformError::Connection(msg.into()))
    }

    /// 创建外部服务错误
    pub fn external(service: impl Into<String>, msg: impl Into<String>) -> Self {
        let s = service.into();
        let m = msg.into();
        match s.as_str() {
            "http" | "reqwest" => XueliError::AIService(AIServiceError::ModelRequest(m)),
            "json" | "serde" => XueliError::Serialization(m),
            _ => XueliError::Internal(format!("[{}] {}", s, m)),
        }
    }

    /// 创建验证错误
    pub fn validation(field: impl Into<String>, msg: impl Into<String>) -> Self {
        XueliError::Config(format!("{}: {}", field.into(), msg.into()))
    }

    /// 分类异常为管线错误类别
    pub fn classify_pipeline_error(err: &XueliError) -> PipelineErrorCategory {
        match err {
            XueliError::Config(_) => PipelineErrorCategory::Configuration,
            XueliError::AIService(
                AIServiceError::ModelRequest(_)
                | AIServiceError::Timeout
                | AIServiceError::RateLimited
                | AIServiceError::ApiError(_),
            ) => PipelineErrorCategory::ModelRequest,
            XueliError::AIService(AIServiceError::ModelParse(_)) => {
                PipelineErrorCategory::ModelParse
            }
            XueliError::AIService(AIServiceError::ImageProcessing(_)) => {
                PipelineErrorCategory::ImageProcessing
            }
            XueliError::Memory(_) => PipelineErrorCategory::MemoryOperation,
            XueliError::Platform(PlatformError::Send(_)) => PipelineErrorCategory::Send,
            XueliError::Platform(_) => PipelineErrorCategory::Platform,
            XueliError::Pipeline { category, .. } => *category,
            _ => PipelineErrorCategory::PipelineExecution,
        }
    }
}
