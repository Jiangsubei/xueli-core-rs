use serde::{Deserialize, Serialize};

/// 聊天作用域 — 对应 Python 版 `ChatScope`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChatScope {
    /// 私聊
    Private,
    /// 群聊
    Group(String),
}

impl ChatScope {
    pub fn is_group(&self) -> bool {
        matches!(self, ChatScope::Group(_))
    }

    pub fn is_private(&self) -> bool {
        matches!(self, ChatScope::Private)
    }

    pub fn group_id(&self) -> Option<&str> {
        match self {
            ChatScope::Group(id) => Some(id),
            ChatScope::Private => None,
        }
    }

    pub fn user_id(&self) -> Option<&str> {
        match self {
            ChatScope::Private => None,
            ChatScope::Group(id) => Some(id),
        }
    }
}
