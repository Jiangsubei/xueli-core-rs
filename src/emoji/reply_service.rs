use crate::emoji::database::EmojiEntry;
use crate::emoji::manager::EmojiManager;

/// 表情追评服务 — 决定是否在回复后追加表情，并选择合适表情。
pub struct EmojiReplyService {
    manager: EmojiManager,
}

impl EmojiReplyService {
    pub fn new(manager: EmojiManager) -> Self {
        Self { manager }
    }

    /// 判断回复后是否应追加表情。
    ///
    /// 规则：短回复（< 30 字）且包含情绪关键词时追加。
    pub fn should_add_emoji(&self, reply_text: &str) -> bool {
        if reply_text.is_empty() {
            return false;
        }
        let char_count = reply_text.chars().count();
        // 长回复不追评
        if char_count > 30 {
            return false;
        }
        // 检测是否有情绪表达
        let has_emotion = [
            "哈哈", "😊", "好", "不错", "棒", "嗯", "哦", "啊", "呀", "嘿嘿", "嘻嘻", "唉", "谢谢",
            "再见", "晚安", "加油", "赞", "❤",
        ]
        .iter()
        .any(|kw| reply_text.contains(kw));

        has_emotion
    }

    /// 选择合适的追评表情
    pub fn pick_emoji(&self, reply_text: &str) -> Option<EmojiEntry> {
        self.manager.recommend(reply_text).ok().flatten()
    }
}

impl Default for EmojiReplyService {
    fn default() -> Self {
        // 默认构造（不含数据）
        let db = crate::emoji::database::EmojiDB::new("data/emojis");
        Self::new(EmojiManager::new(db))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_add_emoji_short_emotional() {
        let svc = EmojiReplyService::default();
        assert!(svc.should_add_emoji("哈哈不错"));
        assert!(svc.should_add_emoji("嗯"));
    }

    #[test]
    fn test_should_not_add_emoji_long() {
        let svc = EmojiReplyService::default();
        assert!(!svc
            .should_add_emoji("这是一个很长的回复内容用来测试是否会自动追评表情功能是否正常工作"));
    }

    #[test]
    fn test_should_not_add_emoji_neutral() {
        let svc = EmojiReplyService::default();
        assert!(!svc.should_add_emoji("今天的会议安排是下午三点在A会议室"));
    }
}
