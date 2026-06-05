use std::collections::HashMap;

/// 会话范围的对话轮次缓冲区，驱动记忆提取的触发时机。
///
/// 每个会话最多保留 200 轮历史，通过记录已提取的轮次来判断是否有足够的新对话可供提取。
#[derive(Debug, Default)]
pub struct ExtractionBuffer {
    /// 按会话键存储的对话轮次列表
    session_turns: HashMap<String, Vec<BufferTurn>>,
    /// 每个会话的归属用户
    session_owner: HashMap<String, String>,
    /// 每个会话的对话键
    session_dialogue_key: HashMap<String, String>,
    /// 每个会话最近一次提取到的轮次编号
    session_extracted_upto: HashMap<String, usize>,
}

/// 缓冲区中缓存的一轮对话（一问一答）
#[derive(Debug, Clone)]
pub struct BufferTurn {
    pub turn_id: usize,
    pub user: String,
    pub assistant: String,
    pub timestamp: String,
    pub source_message_type: String,
    pub source_group_id: String,
    pub source_message_id: String,
    pub source_platform: String,
    pub owner_user_id: String,
    pub dialogue_key: String,
    pub session_id: String,
    pub narrative_summary: String,
}

impl ExtractionBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// 向缓冲区写入一轮对话（用户消息 + 助手回复），自动截断超过 200 轮的历史。
    #[allow(clippy::too_many_arguments)]
    pub fn add_dialogue_turn(
        &mut self,
        user_id: &str,
        user_message: &str,
        assistant_message: &str,
        session_id: &str,
        turn_id: usize,
        dialogue_key: &str,
        message_type: &str,
        group_id: &str,
        message_id: &str,
        narrative_summary: &str,
        source_platform: &str,
    ) {
        let session_key = session_id.trim().to_string();
        if session_key.is_empty() {
            return;
        }

        let turns = self.session_turns.entry(session_key.clone()).or_default();
        // 保留最近 200 轮
        if turns.len() >= 200 {
            let excess = turns.len() - 199;
            turns.drain(..excess);
        }

        self.session_owner
            .insert(session_key.clone(), user_id.to_string());
        self.session_dialogue_key
            .insert(session_key.clone(), dialogue_key.to_string());

        let now = chrono::Utc::now().to_rfc3339();
        turns.push(BufferTurn {
            turn_id,
            user: user_message.to_string(),
            assistant: assistant_message.to_string(),
            timestamp: now,
            source_message_type: if message_type.is_empty() {
                "private".into()
            } else {
                message_type.to_string()
            },
            source_group_id: group_id.to_string(),
            source_message_id: message_id.to_string(),
            source_platform: source_platform.to_string(),
            owner_user_id: user_id.to_string(),
            dialogue_key: dialogue_key.to_string(),
            session_id: session_key,
            narrative_summary: narrative_summary.to_string(),
        });
    }

    /// 判断指定会话是否需要触发提取
    pub fn should_extract(&self, session_id: &str, extract_every_n_turns: usize) -> bool {
        let interval = std::cmp::max(1, extract_every_n_turns);
        self.get_pending_turn_count(session_id) >= interval
    }

    /// 获取指定会话的轮次总数
    pub fn get_turn_count(&self, session_id: &str) -> usize {
        let session_key = session_id.trim();
        self.session_turns
            .get(session_key)
            .map(|t| t.len())
            .unwrap_or(0)
    }

    /// 获取指定会话的待提取轮次数
    pub fn get_pending_turn_count(&self, session_id: &str) -> usize {
        self.get_pending_turns(session_id).len()
    }

    /// 获取指定会话的待提取轮次列表
    pub fn get_pending_turns(&self, session_id: &str) -> Vec<&BufferTurn> {
        let session_key = session_id.trim();
        let turns = match self.session_turns.get(session_key) {
            Some(t) => t,
            None => return vec![],
        };
        let extracted_upto = self
            .session_extracted_upto
            .get(session_key)
            .copied()
            .unwrap_or(0);
        turns
            .iter()
            .filter(|t| t.turn_id > extracted_upto)
            .collect()
    }

    /// 标记会话在指定 turn_id 处已提取
    pub fn mark_session_extracted(&mut self, session_id: &str, turn_id: usize) {
        let session_key = session_id.trim().to_string();
        if session_key.is_empty() {
            return;
        }
        let current = self
            .session_extracted_upto
            .get(&session_key)
            .copied()
            .unwrap_or(0);
        self.session_extracted_upto
            .insert(session_key, std::cmp::max(current, turn_id));
    }

    /// 获取会话的对话键
    pub fn get_dialogue_key(&self, session_key: &str) -> &str {
        self.session_dialogue_key
            .get(session_key)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    /// 获取会话的所有者 user_id
    pub fn get_session_owner(&self, session_key: &str) -> &str {
        self.session_owner
            .get(session_key)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    /// 清空缓冲区
    ///
    /// 若指定 session_id 则仅清该会话；否则清空全部。
    pub fn clear_buffer(&mut self, session_id: Option<&str>) {
        if let Some(sid) = session_id {
            let session_key = sid.trim().to_string();
            if session_key.is_empty() {
                return;
            }
            self.session_turns.remove(&session_key);
            self.session_owner.remove(&session_key);
            self.session_dialogue_key.remove(&session_key);
            self.session_extracted_upto.remove(&session_key);
        } else {
            self.session_turns.clear();
            self.session_owner.clear();
            self.session_dialogue_key.clear();
            self.session_extracted_upto.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_turn(buf: &mut ExtractionBuffer, session: &str, turn_id: usize) {
        buf.add_dialogue_turn(
            "u1", "hello", "hi there", session, turn_id, "dk", "private", "", "", "", "test",
        );
    }

    #[test]
    fn test_add_and_count() {
        let mut buf = ExtractionBuffer::new();
        add_turn(&mut buf, "s1", 1);
        add_turn(&mut buf, "s1", 2);
        assert_eq!(buf.get_turn_count("s1"), 2);
    }

    #[test]
    fn test_pending_turns() {
        let mut buf = ExtractionBuffer::new();
        add_turn(&mut buf, "s1", 1);
        add_turn(&mut buf, "s1", 2);
        add_turn(&mut buf, "s1", 3);

        // 尚未标记任何提取
        assert_eq!(buf.get_pending_turn_count("s1"), 3);

        // 标记 turn 2 已提取
        buf.mark_session_extracted("s1", 2);
        assert_eq!(buf.get_pending_turn_count("s1"), 1);
        let pending = buf.get_pending_turns("s1");
        assert_eq!(pending[0].turn_id, 3);
    }

    #[test]
    fn test_should_extract() {
        let mut buf = ExtractionBuffer::new();
        assert!(!buf.should_extract("s1", 5));
        for i in 1..=6 {
            add_turn(&mut buf, "s1", i);
        }
        assert!(buf.should_extract("s1", 5));
        assert!(!buf.should_extract("s1", 7));
    }

    #[test]
    fn test_max_turns_200() {
        let mut buf = ExtractionBuffer::new();
        for i in 0..250 {
            add_turn(&mut buf, "s1", i);
        }
        assert!(buf.get_turn_count("s1") <= 200);
        // 应保留最近 200 轮，即 turn_id 50..250
        let pending = buf.get_pending_turns("s1");
        assert!(pending.len() <= 200);
    }

    #[test]
    fn test_clear_single_session() {
        let mut buf = ExtractionBuffer::new();
        add_turn(&mut buf, "s1", 1);
        add_turn(&mut buf, "s2", 1);
        buf.clear_buffer(Some("s1"));
        assert_eq!(buf.get_turn_count("s1"), 0);
        assert_eq!(buf.get_turn_count("s2"), 1);
    }

    #[test]
    fn test_clear_all() {
        let mut buf = ExtractionBuffer::new();
        add_turn(&mut buf, "s1", 1);
        add_turn(&mut buf, "s2", 1);
        buf.clear_buffer(None);
        assert_eq!(buf.get_turn_count("s1"), 0);
        assert_eq!(buf.get_turn_count("s2"), 0);
    }

    #[test]
    fn test_empty_session_id_is_ignored() {
        let mut buf = ExtractionBuffer::new();
        buf.add_dialogue_turn("u1", "a", "b", "", 1, "", "private", "", "", "", "");
        assert_eq!(buf.get_turn_count(""), 0);
    }
}
