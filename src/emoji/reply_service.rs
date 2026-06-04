use crate::emoji::database::EmojiEntry;

/// 表情追评服务 — 决定是否在回复后追加表情
pub struct EmojiReplyService;

impl EmojiReplyService {
    pub fn new() -> Self {
        Self
    }

    /// 决定是否追加表情回复
    pub fn should_add_emoji(&self, _reply_text: &str) -> bool {
        // TODO: 基于回复内容判断
        false
    }

    /// 选择合适的追评表情
    pub fn pick_emoji(&self, _reply_text: &str) -> Option<EmojiEntry> {
        // TODO: 选择合适表情
        None
    }
}

impl Default for EmojiReplyService {
    fn default() -> Self {
        Self::new()
    }
}
