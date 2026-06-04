/// 消息参与度观察 — 构建中性消息观察向量
///
/// 只描述可观测事实，不输出语义判断结论。
/// 所有语义判断由 planner 和 timing gate 基于这些观察自行决策。
///
/// 对应 Python 版 `xueli/src/handlers/conversation/engagement.py`
use std::collections::HashSet;

/// 轻量回应候选词（通常不需要深度回复的短消息）
const LIGHT_RESPONSE_TOKENS: &[&str] = &[
    "嗯",
    "嗯嗯",
    "好哦",
    "好耶",
    "原来如此",
    "这样啊",
    "懂了",
    "然后呢",
    "继续",
    "后来呢",
];

/// 延续候选词（表示希望 bot 继续展开的追问）
const CONTINUATION_TOKENS: &[&str] = &[
    "然后呢",
    "后来呢",
    "后来",
    "继续",
    "接着",
    "结果呢",
    "结果",
    "再然后",
    "再说说",
    "展开讲",
    "那现在",
    "那后来",
    "所以呢",
    "所以",
    "还有呢",
    "细说",
];

/// 消息观察结果
#[derive(Debug, Clone)]
pub struct MessageObservations {
    pub message_length_bucket: String,
    pub is_short_message: bool,
    pub is_light_response_candidate: bool,
    pub is_continuation_candidate: bool,
    pub assistant_replied_recently: bool,
    pub follows_assistant_recently: bool,
    pub same_user_continuation: bool,
    pub recent_history_count: usize,
    pub latest_message_length: usize,
}

impl Default for MessageObservations {
    fn default() -> Self {
        Self {
            message_length_bucket: "ultra_short".to_string(),
            is_short_message: true,
            is_light_response_candidate: false,
            is_continuation_candidate: false,
            assistant_replied_recently: false,
            follows_assistant_recently: false,
            same_user_continuation: false,
            recent_history_count: 0,
            latest_message_length: 0,
        }
    }
}

/// 规范化消息文本（去空格，转小写）
pub fn normalize_engagement_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

/// 消息长度分桶
fn message_length_bucket(text: &str) -> String {
    let len = text.chars().count();
    if len <= 3 {
        "ultra_short".to_string()
    } else if len <= 8 {
        "short".to_string()
    } else if len <= 20 {
        "medium".to_string()
    } else {
        "long".to_string()
    }
}

/// 判断是否为轻量回应候选
fn is_light_response_candidate(text: &str) -> bool {
    let normalized = normalize_engagement_text(text);
    if normalized.is_empty() {
        return false;
    }
    normalized.len() <= 6 && LIGHT_RESPONSE_TOKENS.contains(&normalized.as_str())
}

/// 判断是否为延续候选
fn is_continuation_candidate(text: &str) -> bool {
    let normalized = normalize_engagement_text(text);
    if normalized.is_empty() {
        return false;
    }
    CONTINUATION_TOKENS
        .iter()
        .any(|token| normalized.contains(token))
}

/// 构建中性消息观察向量
///
/// # 参数
/// - `text`: 当前消息文本
/// - `current_user_id`: 当前消息发送者
/// - `previous_speaker_role`: 上一条消息的发言角色（"user" / "assistant"）
/// - `previous_user_id`: 上一条用户消息的发送者
/// - `recent_gap_bucket`: 最近消息间隔分桶
/// - `recent_history_count`: 最近历史消息数
pub fn build_message_observations(
    text: &str,
    current_user_id: &str,
    previous_speaker_role: &str,
    previous_user_id: &str,
    recent_gap_bucket: &str,
    recent_history_count: usize,
) -> MessageObservations {
    let normalized = normalize_engagement_text(text);
    let prev_role = previous_speaker_role.to_lowercase();

    let recent_buckets: HashSet<&str> = ["immediate", "very_recent", "recent"]
        .iter()
        .cloned()
        .collect();
    let assistant_replied_recently =
        prev_role == "assistant" && recent_buckets.contains(recent_gap_bucket);

    let same_user_continuation = prev_role == "user"
        && !current_user_id.is_empty()
        && current_user_id == previous_user_id
        && is_continuation_candidate(text);

    let follows_assistant_recently = assistant_replied_recently
        && (is_continuation_candidate(text) || normalized.chars().count() <= 8);

    MessageObservations {
        message_length_bucket: message_length_bucket(&normalized),
        is_short_message: normalized.chars().count() <= 8,
        is_light_response_candidate: is_light_response_candidate(text),
        is_continuation_candidate: is_continuation_candidate(text),
        assistant_replied_recently,
        follows_assistant_recently,
        same_user_continuation,
        recent_history_count: recent_history_count.max(0),
        latest_message_length: normalized.chars().count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_engagement_text() {
        let result = normalize_engagement_text(" 你好  世界 ");
        assert_eq!(result, "你好世界");
    }

    #[test]
    fn test_message_length_bucket_ultra_short() {
        assert_eq!(message_length_bucket("嗯"), "ultra_short");
    }

    #[test]
    fn test_message_length_bucket_short() {
        assert_eq!(message_length_bucket("你好啊今天"), "short");
    }

    #[test]
    fn test_message_length_bucket_long() {
        assert_eq!(
            message_length_bucket("这是一个很长很长很长很长很长很长很长很长很长的消息"),
            "long"
        );
    }

    #[test]
    fn test_light_response_candidate_positive() {
        assert!(is_light_response_candidate("嗯嗯"));
        assert!(is_light_response_candidate("懂了"));
    }

    #[test]
    fn test_light_response_candidate_negative() {
        assert!(!is_light_response_candidate("今天天气真好"));
    }

    #[test]
    fn test_continuation_candidate_positive() {
        assert!(is_continuation_candidate("然后呢"));
        assert!(is_continuation_candidate("展开讲讲"));
    }

    #[test]
    fn test_continuation_candidate_negative() {
        assert!(!is_continuation_candidate("今天吃了吗"));
    }

    #[test]
    fn test_build_message_observations_assistant_replied() {
        let obs = build_message_observations("然后呢", "u1", "assistant", "u1", "immediate", 5);
        assert!(obs.follows_assistant_recently);
        assert!(obs.assistant_replied_recently);
    }

    #[test]
    fn test_build_message_observations_same_user_continuation() {
        let obs = build_message_observations("继续", "u1", "user", "u1", "very_recent", 3);
        assert!(obs.same_user_continuation);
    }

    #[test]
    fn test_build_message_observations_different_user() {
        let obs = build_message_observations("继续", "u1", "user", "u2", "recent", 3);
        assert!(!obs.same_user_continuation);
    }

    #[test]
    fn test_build_message_observations_empty_text() {
        let obs = build_message_observations("", "u1", "", "", "unknown", 0);
        assert_eq!(obs.latest_message_length, 0);
        assert!(!obs.is_light_response_candidate);
    }
}
