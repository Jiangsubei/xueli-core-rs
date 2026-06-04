use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::scope::ChatScope;
use crate::core::types::UserMessage;

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
