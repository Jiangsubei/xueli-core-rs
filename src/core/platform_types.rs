use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::scope::ChatScope;
use crate::core::types::UserMessage;

/// 消息段
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageSegment {
    #[serde(default, rename = "type")]
    pub segment_type: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// 附件引用
///
/// 对应 Python 版 `src.core.platform_models.AttachmentRef`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AttachmentRef {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub attachment_id: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// 发送者引用
///
/// 对应 Python 版 `src.core.platform_models.SenderRef`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SenderRef {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub platform_user_id: String,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// 平台能力
///
/// 对应 Python 版 `src.core.platform_models.PlatformCapabilities`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlatformCapabilities {
    #[serde(default = "default_true")]
    pub supports_text: bool,
    #[serde(default)]
    pub supports_images: bool,
    #[serde(default)]
    pub supports_face: bool,
    #[serde(default)]
    pub supports_mface: bool,
    #[serde(default)]
    pub supports_quote_reply: bool,
    #[serde(default)]
    pub supports_groups: bool,
    #[serde(default)]
    pub supports_message_edit: bool,
    #[serde(default)]
    pub supports_files: bool,
    #[serde(default)]
    pub supports_proactive_push: bool,
    #[serde(default)]
    pub supports_voice: bool,
    #[serde(default)]
    pub supports_video: bool,
    #[serde(default)]
    pub supports_cards: bool,
    #[serde(default)]
    pub supports_stickers: bool,
    #[serde(default)]
    pub max_message_length: Option<usize>,
}

fn default_true() -> bool {
    true
}

/// 平台入站事件（统一抽象）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundEvent {
    /// 事件 ID
    pub id: String,
    /// 平台名称
    pub platform: String,
    /// 事件类型
    pub event_type: EventType,
    /// 消息内容
    pub message: Option<UserMessage>,
    /// 原始载荷
    pub raw_payload: Option<String>,
    /// 接收时间
    pub received_at: DateTime<Utc>,
    /// 会话引用（由适配器填充，若缺失从 message 推导）
    pub session: Option<SessionRef>,
    /// 被提及的用户 ID 列表
    #[serde(default)]
    pub mentioned_user_ids: Vec<String>,
    /// 消息种类 (text/image/mixed/forward/sticker/voice/video/file/card/unknown)
    #[serde(default)]
    pub message_kind: String,
    /// 消息段
    #[serde(default)]
    pub segments: Vec<MessageSegment>,
    /// 附件列表
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,
    /// 发送者信息
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<SenderRef>,
    /// 平台能力
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<PlatformCapabilities>,
    /// 消息原始文本
    #[serde(default)]
    pub text: String,
    /// 清洗后的文本
    #[serde(default)]
    pub clean_text: String,
    /// 事件时间戳（Unix 秒）
    #[serde(default)]
    pub timestamp: f64,
    /// 回复目标消息 ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
    /// 时间间隔分桶（由调度器/上下文构建阶段填充，用于 Drive 事件模式提取）
    #[serde(default)]
    pub temporal_gap_bucket: String,
    /// 用户反馈标签（由效果追踪器填充，用于 Drive 事件模式提取）
    #[serde(default)]
    pub feedback_label: String,
}

impl InboundEvent {
    /// 从事件信息构造或获取 SessionRef
    pub fn get_session(&self) -> SessionRef {
        if let Some(ref s) = self.session {
            return s.clone();
        }
        let (scope, user_id) = if let Some(ref msg) = self.message {
            (msg.scope.clone(), msg.sender_id.clone())
        } else {
            (ChatScope::Private, String::new())
        };
        let session_id = match &scope {
            ChatScope::Private => format!("{}:private:{}", self.platform, user_id),
            ChatScope::Group(gid) => format!("{}:group:{}", self.platform, gid),
        };
        SessionRef {
            session_id,
            scope,
            user_id: if user_id.is_empty() {
                None
            } else {
                Some(user_id)
            },
        }
    }
}

impl Default for InboundEvent {
    fn default() -> Self {
        Self {
            id: String::new(),
            platform: String::new(),
            event_type: EventType::Other(String::new()),
            message: None,
            raw_payload: None,
            received_at: Utc::now(),
            session: None,
            mentioned_user_ids: Vec::new(),
            message_kind: String::new(),
            segments: Vec::new(),
            attachments: Vec::new(),
            sender: None,
            capabilities: None,
            text: String::new(),
            clean_text: String::new(),
            timestamp: 0.0,
            reply_to_message_id: None,
            temporal_gap_bucket: String::new(),
            feedback_label: String::new(),
        }
    }
}

/// 事件类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventType {
    Message,
    Mention,
    JoinGroup,
    LeaveGroup,
    Heartbeat,
    Other(String),
}

/// 回复动作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyAction {
    /// 目标作用域
    pub scope: ChatScope,
    /// 回复文本
    pub text: String,
    /// 引用消息 ID
    pub reply_to: Option<String>,
    /// 附加图片 URL
    pub image_url: Option<String>,
    /// 附加表情 ID
    pub emoji_id: Option<String>,
}

/// 会话引用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRef {
    /// 会话 ID
    pub session_id: String,
    /// 作用域
    pub scope: ChatScope,
    /// 用户 ID
    pub user_id: Option<String>,
}

/// 发送动作（平台级操作）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SendAction {
    /// 发送消息
    SendMessage(ReplyAction),
    /// 设置"正在输入"状态
    SetTyping(SessionRef, bool),
    /// 标记消息已读
    MarkRead(SessionRef),
}

/// 出站动作（平台无关）
///
/// 对应 Python 版 `src.core.platform_models.OutgoingAction` 及其子类
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutgoingAction {
    SendMessage {
        #[serde(default)]
        text: String,
        #[serde(default)]
        segments: Vec<MessageSegment>,
    },
    SendSticker {
        #[serde(default)]
        file_id: String,
    },
    Noop,
}

/// 群聊状态机状态
///
/// 对应 Python 版 `GroupState`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupState {
    /// 正常运行，处理消息
    Running,
    /// 等待更多上下文，消息仅缓冲
    Waiting,
    /// 已停止，忽略消息直到重新唤醒
    Stopped,
}

impl GroupState {
    pub fn as_str(&self) -> &'static str {
        match self {
            GroupState::Running => "running",
            GroupState::Waiting => "waiting",
            GroupState::Stopped => "stopped",
        }
    }
}
