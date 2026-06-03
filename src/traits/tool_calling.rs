use async_trait::async_trait;

/// 工具调用定义
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
}

/// 工具调用请求
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// 工具调用结果
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
}

/// ToolCalling 策略 trait — 下游实现 LLM 协议特有的工具调用解析
#[async_trait]
pub trait ToolCallingStrategy: Send + Sync {
    /// 从 LLM 响应文本中解析工具调用
    fn parse_tool_calls(&self, response_text: &str) -> Result<Vec<ToolCall>, String>;

    /// 将工具定义序列化为协议特有的格式
    fn serialize_tools(&self, tools: &[ToolDefinition]) -> Result<serde_json::Value, String>;
}