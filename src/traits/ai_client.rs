use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::prelude::XueliResult;

// ── 消息内容类型 ──────────────────────────────────────────

/// 多模态内容片段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    /// 文本片段
    #[serde(rename = "text")]
    Text { text: String },
    /// 图片片段
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlPayload },
}

/// 图片 URL 载荷
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrlPayload {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 消息内容 — 支持纯文本与多模态
#[derive(Debug, Clone)]
pub enum MessageContent {
    /// 纯文本
    Text(String),
    /// 多模态（文本 + 图片混排）
    Multimodal(Vec<ContentPart>),
}

impl MessageContent {
    /// 提取纯文本内容（多模态时拼接所有 text 片段）
    pub fn text(&self) -> String {
        match self {
            MessageContent::Text(s) => s.clone(),
            MessageContent::Multimodal(parts) => parts
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// 是否含图片
    pub fn has_images(&self) -> bool {
        match self {
            MessageContent::Multimodal(parts) => parts
                .iter()
                .any(|p| matches!(p, ContentPart::ImageUrl { .. })),
            _ => false,
        }
    }
}

impl From<String> for MessageContent {
    fn from(s: String) -> Self {
        MessageContent::Text(s)
    }
}

impl From<&str> for MessageContent {
    fn from(s: &str) -> Self {
        MessageContent::Text(s.to_string())
    }
}

// 自定义序列化：Text → 字符串，Multimodal → 数组
impl Serialize for MessageContent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            MessageContent::Text(s) => serializer.serialize_str(s),
            MessageContent::Multimodal(parts) => parts.serialize(serializer),
        }
    }
}

// 自定义反序列化：字符串 → Text，数组 → Multimodal
impl<'de> Deserialize<'de> for MessageContent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de;

        struct MessageContentVisitor;
        impl<'de> de::Visitor<'de> for MessageContentVisitor {
            type Value = MessageContent;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string or an array of content parts")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(MessageContent::Text(v.to_string()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(MessageContent::Text(v))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut parts = Vec::new();
                while let Some(part) = seq.next_element::<ContentPart>()? {
                    parts.push(part);
                }
                Ok(MessageContent::Multimodal(parts))
            }
        }

        deserializer.deserialize_any(MessageContentVisitor)
    }
}

// ── AI 模型消息 ──────────────────────────────────────────

/// AI 模型消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    /// 构造纯文本消息
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: MessageContent::Text(content.into()),
            name: None,
        }
    }

    /// 构造多模态消息（文本 + base64 图片列表）
    pub fn multimodal(
        role: impl Into<String>,
        text: impl Into<String>,
        images: &[String],
        image_format: &str,
    ) -> Self {
        let mut parts: Vec<ContentPart> = Vec::new();
        let text_str = text.into();
        if !text_str.is_empty() {
            parts.push(ContentPart::Text { text: text_str });
        }
        for image_data in images {
            let url = if image_data.starts_with("data:") {
                image_data.clone()
            } else {
                format!("data:{};base64,{}", image_format, image_data)
            };
            parts.push(ContentPart::ImageUrl {
                image_url: ImageUrlPayload { url, detail: None },
            });
        }
        Self {
            role: role.into(),
            content: MessageContent::Multimodal(parts),
            name: None,
        }
    }
}

// ── 请求 / 响应 ──────────────────────────────────────────

/// AI 模型聊天请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    /// 模型名称
    pub model: String,
    /// 对话消息列表
    pub messages: Vec<ChatMessage>,
    /// temperature（None 使用配置默认值）
    pub temperature: Option<f64>,
    /// max_tokens（None 使用配置默认值）
    pub max_tokens: Option<u32>,
    /// 是否流式
    #[serde(default)]
    pub stream: bool,
    /// 额外参数（透传到请求体顶层）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra_params: HashMap<String, serde_json::Value>,
}

/// AI 模型聊天响应（归一化后的稳定结构）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    /// 主回复文本
    pub content: String,
    /// 分段（用于分条发送）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<String>>,
    /// 推理内容（推理模型如 o1 系列）
    #[serde(default)]
    pub reasoning_content: String,
    /// 完成原因
    #[serde(default)]
    pub finish_reason: String,
    /// Token 用量
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// 实际使用的模型名称
    #[serde(default)]
    pub model: String,
    /// 工具调用列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Token 用量统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// 工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_type")]
    pub call_type: String,
    pub function: FunctionCall,
}

fn default_tool_type() -> String {
    "function".to_string()
}

/// 函数调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

// ── Trait ────────────────────────────────────────────────

/// AI 客户端 trait — 下游通过实现此 trait 接入不同的 AI 服务
#[async_trait]
pub trait AIClient: Send + Sync {
    /// 发送聊天补全请求，返回归一化响应
    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> XueliResult<ChatCompletionResponse>;
}
