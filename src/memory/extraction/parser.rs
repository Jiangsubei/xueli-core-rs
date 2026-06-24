use std::collections::HashSet;

use crate::memory::extraction::models::ExtractedMemory;

const VALID_TONES: &[&str] = &[
    "开心", "喜欢", "惊讶", "无语", "委屈", "生气", "伤心", "嘲讽", "害怕", "困惑", "平静",
];

const CATEGORY_MAP: &[(&str, &str)] = &[
    ("核心事实", "core_fact"),
    ("重要", "important"),
    ("闲聊", "casual"),
];

const NO_MEMORY_PATTERNS: &[&str] = &[
    "暂无记忆",
    "no memory",
    "无相关记忆",
    "未发现",
    "没有值得",
    "无需记录",
    "无需提取",
    "没有相关信息",
    "无记忆",
    "no relevant memory",
    "nothing to remember",
    "no significant",
    "no meaningful",
    "trivial",
    "nothing worth",
    "nothing notable",
    "no new information",
    "信息不足",
    "无法确定",
    "不足以提取",
    "无有效信息",
    "没有可提取",
    "当前对话",
];

const NO_MEMORY_EXACT: &[(&str, &str)] = &[
    ("无", ""),
    ("无可提取内容", ""),
    ("没有可提取内容", ""),
    ("暂无可提取内容", ""),
    ("none", ""),
    ("nomemory", ""),
    ("nomemories", ""),
    ("noextractablememory", ""),
    ("noextractablememories", ""),
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReflectionParseResult {
    pub has_conflict: bool,
    pub conflict_type: String,
    pub action: String,
    pub summary: String,
    pub reason: String,
    pub confidence: f64,
}

pub fn parse_extraction_response(content: &str) -> Vec<ExtractedMemory> {
    let mut memories: Vec<ExtractedMemory> = Vec::new();
    let text = content.trim();
    if text.is_empty() {
        return memories;
    }

    let lines: Vec<String> = if text.contains('|') && text.lines().count() == 1 {
        let has_directive_segments = text.split('|').any(|s| s.trim().starts_with('['));
        if has_directive_segments {
            text.split('|')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            vec![text.to_string()]
        }
    } else {
        text.lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    let mut current_tone = String::new();
    let mut current_category = String::from("important");
    let mut current_fact_kind = String::new();

    for raw_line in &lines {
        let line = raw_line.as_str();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if is_explicit_no_memory_response(line) {
            continue;
        }

        if let Some(tone) = try_parse_tone_directive(line) {
            current_tone = tone;
            continue;
        }

        if let Some(category) = try_parse_category_directive(line) {
            current_category = category;
            continue;
        }

        if let Some(fact_kind) = try_parse_fact_kind_directive(line) {
            current_fact_kind = fact_kind;
            continue;
        }

        if let Some(memory) =
            try_parse_pipe_format(line, &current_tone, &current_category, &current_fact_kind)
        {
            memories.push(memory);
            continue;
        }

        if let Some(memory) =
            try_parse_tagged_format(line, &current_tone, &current_category, &current_fact_kind)
        {
            memories.push(memory);
        }
    }

    memories
}

pub fn is_valid_anchor(memory: &ExtractedMemory, allowed_turn_ids: &HashSet<u32>) -> bool {
    if memory.source_turn_start == 0 || memory.source_turn_end == 0 {
        return false;
    }
    if memory.source_turn_start > memory.source_turn_end {
        return false;
    }
    (memory.source_turn_start..=memory.source_turn_end).all(|t| allowed_turn_ids.contains(&t))
}

pub fn parse_anchor(anchor: &str) -> Option<(u32, u32)> {
    let s = anchor.trim();
    if s.is_empty() {
        return None;
    }

    let s_upper = s.to_uppercase();
    let without_t = s_upper.strip_prefix('T').unwrap_or(&s_upper);

    if let Some((start_str, end_str)) = without_t.split_once('-') {
        let end_clean = end_str.strip_prefix('T').unwrap_or(end_str);
        let start: u32 = start_str.parse().ok()?;
        let end: u32 = end_clean.parse().ok()?;
        if start == 0 || end == 0 || start > end {
            return None;
        }
        Some((start, end))
    } else {
        let id: u32 = without_t.parse().ok()?;
        if id == 0 {
            return None;
        }
        Some((id, id))
    }
}

pub fn is_explicit_no_memory_response(content: &str) -> bool {
    let text = content.trim();
    if text.is_empty() {
        return false;
    }

    let normalized = normalize_text_simple(text);
    for (pattern, _) in NO_MEMORY_EXACT {
        if normalized == *pattern {
            return true;
        }
    }

    let lower = text.to_lowercase();
    NO_MEMORY_PATTERNS
        .iter()
        .any(|pat| lower.contains(&pat.to_lowercase()))
}

pub fn is_rate_limit_error(error_message: &str) -> bool {
    let msg = error_message.to_lowercase();
    msg.contains("429") || msg.contains("rate limit") || msg.contains("rate-limited")
}

pub fn parse_reflection_response(content: &str) -> Option<ReflectionParseResult> {
    let payload = extract_json_object(content);
    if payload.is_empty() {
        return None;
    }

    let data: serde_json::Value = serde_json::from_str(&payload).ok()?;

    let has_conflict = data
        .get("has_conflict")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let conflict_type = data
        .get("conflict_type")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("none")
        .to_string();

    let action = data
        .get("action")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("keep_both")
        .to_string();

    let summary = data
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let reason = data
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let confidence = data
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);

    Some(ReflectionParseResult {
        has_conflict,
        conflict_type,
        action,
        summary,
        reason,
        confidence,
    })
}

fn extract_json_object(content: &str) -> String {
    let text = content.trim();
    if text.is_empty() {
        return String::new();
    }
    if text.starts_with('{') && text.ends_with('}') {
        return text.to_string();
    }
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            return text[start..=end].to_string();
        }
    }
    String::new()
}

fn normalize_text_simple(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !matches!(
                c,
                ' ' | '\t'
                    | '\n'
                    | '\r'
                    | '`'
                    | '\''
                    | '"'
                    | '“'
                    | '”'
                    | '‘'
                    | '’'
                    | '。'
                    | '．'
                    | '.'
                    | '!'
                    | '！'
                    | '?'
                    | '？'
                    | '、'
                    | ','
                    | '：'
                    | ':'
                    | '；'
                    | ';'
                    | '-'
                    | '*'
                    | '—'
            )
        })
        .collect::<String>()
        .to_lowercase()
}

fn try_parse_tone_directive(line: &str) -> Option<String> {
    let s = line.trim();
    let s_lower = s.to_lowercase();
    if !s_lower.starts_with("[tone:") || !s.ends_with(']') {
        return None;
    }
    let inner = &s[6..s.len() - 1];
    let tone = inner.trim();
    if VALID_TONES.contains(&tone) {
        Some(tone.to_string())
    } else {
        None
    }
}

fn try_parse_category_directive(line: &str) -> Option<String> {
    let s = line.trim();
    let s_lower = s.to_lowercase();
    let (inner, _) = if s_lower.starts_with("[category:") && s.ends_with(']') {
        (&s[10..s.len() - 1], false)
    } else if s_lower.starts_with("[类别:") && s.ends_with(']') {
        let prefix_end = s.char_indices().nth(4).map(|(i, _)| i).unwrap_or(s.len());
        (&s[prefix_end..s.len() - 1], false)
    } else {
        return None;
    };

    let label = inner.trim();
    for (cn, en) in CATEGORY_MAP {
        if label == *cn {
            return Some(en.to_string());
        }
    }
    None
}

fn try_parse_fact_kind_directive(line: &str) -> Option<String> {
    let s = line.trim();
    let s_lower = s.to_lowercase();
    if !s_lower.starts_with("[fact_kind:") || !s.ends_with(']') {
        return None;
    }
    let inner = &s[11..s.len() - 1];
    let kind = inner.trim();
    let valid_kinds = ["偏好", "边界", "计划", "背景", "档案"];
    if valid_kinds.contains(&kind) {
        Some(kind.to_string())
    } else {
        None
    }
}

fn try_parse_pipe_format(
    line: &str,
    default_tone: &str,
    default_category: &str,
    default_fact_kind: &str,
) -> Option<ExtractedMemory> {
    let s = line.trim();
    if s.starts_with('[') {
        return None;
    }
    if !s.contains('|') {
        return None;
    }

    let parts: Vec<&str> = s.split('|').collect();
    if parts.len() < 3 {
        return None;
    }

    let content = parts[0].trim();
    if content.is_empty() || content.len() <= 2 {
        return None;
    }

    let anchor_start_str = parts[1].trim();
    let anchor_end_str = if parts.len() > 2 {
        parts[2].trim()
    } else {
        anchor_start_str
    };

    let (source_turn_start, _) = parse_anchor(anchor_start_str)?;
    let (source_turn_end, _) = if anchor_end_str.is_empty() || anchor_end_str == anchor_start_str {
        (source_turn_start, source_turn_start)
    } else {
        let (end, _) = parse_anchor(anchor_end_str)?;
        (end, end)
    };

    let memory_category = if parts.len() > 3 && !parts[3].trim().is_empty() {
        parts[3].trim().to_string()
    } else {
        default_category.to_string()
    };

    let emotional_tone = if parts.len() > 4 && !parts[4].trim().is_empty() {
        parts[4].trim().to_string()
    } else {
        default_tone.to_string()
    };

    let importance_f64 = if parts.len() > 5 {
        parts[5]
            .trim()
            .parse::<f64>()
            .unwrap_or(0.5)
            .clamp(0.0, 1.0)
    } else {
        0.5
    };

    let importance = ((importance_f64 * 5.0).round() as u32).clamp(1, 5);
    let is_important = importance >= 5;

    let fact_kind = if parts.len() > 6 && !parts[6].trim().is_empty() {
        parts[6].trim().to_string()
    } else if !default_fact_kind.is_empty() {
        default_fact_kind.to_string()
    } else {
        String::new()
    };

    Some(ExtractedMemory {
        content: content.to_string(),
        source_turn_start,
        source_turn_end,
        is_important,
        importance,
        emotional_tone,
        memory_category,
        fact_kind,
    })
}

fn try_parse_tagged_format(
    line: &str,
    default_tone: &str,
    default_category: &str,
    default_fact_kind: &str,
) -> Option<ExtractedMemory> {
    let s = line.trim();

    let cleaned = strip_memory_prefix(s);

    let important_re =
        regex::Regex::new(r"(?i)^\[(?:IMPORTANT|重要)\]\s*\[(T\d+(?:-T?\d+)?)\]\s*(.+)$").ok()?;

    let normal_re = regex::Regex::new(
        r"(?i)^\[(?:NORMAL|普通)\s*:\s*([1-5])\]\s*\[(T\d+(?:-T?\d+)?)\]\s*(.+)$",
    )
    .ok()?;

    let (is_important, importance, anchor_str, content_text): (bool, u32, &str, &str) =
        if let Some(caps) = important_re.captures(&cleaned) {
            let anchor = caps.get(1)?.as_str();
            let content = caps.get(2)?.as_str();
            (true, 5, anchor, content)
        } else if let Some(caps) = normal_re.captures(&cleaned) {
            let level: u32 = caps.get(1)?.as_str().parse().ok()?;
            let anchor = caps.get(2)?.as_str();
            let content = caps.get(3)?.as_str();
            (false, level, anchor, content)
        } else {
            return None;
        };

    let (start, end) = parse_anchor(anchor_str)?;

    let content_clean = strip_user_prefix(content_text).trim().to_string();
    if content_clean.is_empty() || content_clean.len() <= 2 {
        return None;
    }

    Some(ExtractedMemory {
        content: content_clean,
        source_turn_start: start,
        source_turn_end: end,
        is_important,
        importance,
        emotional_tone: default_tone.to_string(),
        memory_category: default_category.to_string(),
        fact_kind: if !default_fact_kind.is_empty() {
            default_fact_kind.to_string()
        } else {
            String::new()
        },
    })
}

fn strip_memory_prefix(text: &str) -> String {
    let re = regex::Regex::new(r"^(?:普通记忆|重要记忆)\s*[：:]\s*").unwrap();
    let s = re.replace(text, "").to_string();

    let list_re = regex::Regex::new(r"^(?:[\-\*\u{2022}]\s*|\d+[\.\)、]\s*)+").unwrap();
    list_re.replace(&s, "").to_string()
}

fn strip_user_prefix(text: &str) -> String {
    let re = regex::Regex::new(r#"(?i)^(?:用户|user)\s*[^\r\n：:]{1,40}[：:]"#).unwrap();
    re.replace(text, "").to_string()
}

pub fn normalize_reflection_text(text: &str) -> String {
    use regex::Regex;
    Regex::new(r"\s+")
        .unwrap()
        .replace_all(text.trim(), "")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_anchor_single() {
        assert_eq!(parse_anchor("T1"), Some((1, 1)));
        assert_eq!(parse_anchor("T10"), Some((10, 10)));
    }

    #[test]
    fn test_parse_anchor_range() {
        assert_eq!(parse_anchor("T1-T3"), Some((1, 3)));
        assert_eq!(parse_anchor("T1-T10"), Some((1, 10)));
    }

    #[test]
    fn test_parse_anchor_range_with_optional_t() {
        assert_eq!(parse_anchor("T1-T3"), Some((1, 3)));
    }

    #[test]
    fn test_parse_anchor_range_no_t_prefix_end() {
        assert_eq!(parse_anchor("T1-3"), Some((1, 3)));
    }

    #[test]
    fn test_parse_anchor_invalid() {
        assert_eq!(parse_anchor(""), None);
        assert_eq!(parse_anchor("T0"), None);
        assert_eq!(parse_anchor("T3-T1"), None);
        assert_eq!(parse_anchor("abc"), None);
        assert_eq!(parse_anchor("TX"), None);
    }

    #[test]
    fn test_parse_anchor_lowercase() {
        assert_eq!(parse_anchor("t1"), Some((1, 1)));
        assert_eq!(parse_anchor("t1-t3"), Some((1, 3)));
    }

    #[test]
    fn test_is_valid_anchor_basic() {
        let mem = ExtractedMemory {
            content: "test".into(),
            source_turn_start: 1,
            source_turn_end: 3,
            is_important: false,
            importance: 3,
            emotional_tone: String::new(),
            memory_category: "important".into(),
            fact_kind: String::new(),
        };
        let allowed: HashSet<u32> = [1, 2, 3, 4, 5].into_iter().collect();
        assert!(is_valid_anchor(&mem, &allowed));

        let mem2 = ExtractedMemory {
            content: "test".into(),
            source_turn_start: 1,
            source_turn_end: 6,
            is_important: false,
            importance: 3,
            emotional_tone: String::new(),
            memory_category: String::new(),
            fact_kind: String::new(),
        };
        assert!(!is_valid_anchor(&mem2, &allowed));

        let mem3 = ExtractedMemory {
            content: "test".into(),
            source_turn_start: 0,
            source_turn_end: 3,
            is_important: false,
            importance: 3,
            emotional_tone: String::new(),
            memory_category: String::new(),
            fact_kind: String::new(),
        };
        assert!(!is_valid_anchor(&mem3, &allowed));

        let mem4 = ExtractedMemory {
            content: "test".into(),
            source_turn_start: 3,
            source_turn_end: 1,
            is_important: false,
            importance: 3,
            emotional_tone: String::new(),
            memory_category: String::new(),
            fact_kind: String::new(),
        };
        assert!(!is_valid_anchor(&mem4, &allowed));
    }

    #[test]
    fn test_no_memory_exact_match() {
        assert!(is_explicit_no_memory_response("无"));
        assert!(is_explicit_no_memory_response("无可提取内容"));
        assert!(is_explicit_no_memory_response("none"));
        assert!(is_explicit_no_memory_response("nomemory"));
    }

    #[test]
    fn test_no_memory_substring_match() {
        assert!(is_explicit_no_memory_response("暂无记忆"));
        assert!(is_explicit_no_memory_response("no memory found"));
        assert!(is_explicit_no_memory_response("没有值得记录的内容"));
        assert!(is_explicit_no_memory_response("无需记录"));
        assert!(is_explicit_no_memory_response("无需提取"));
        assert!(is_explicit_no_memory_response("没有相关信息"));
        assert!(is_explicit_no_memory_response("nothing to remember here"));
        assert!(is_explicit_no_memory_response("trivial conversation"));
        assert!(is_explicit_no_memory_response("nothing worth noting"));
        assert!(is_explicit_no_memory_response("nothing notable"));
        assert!(is_explicit_no_memory_response("no new information"));
        assert!(is_explicit_no_memory_response("信息不足，无法确定"));
        assert!(is_explicit_no_memory_response("不足以提取记忆"));
        assert!(is_explicit_no_memory_response("无有效信息"));
        assert!(is_explicit_no_memory_response("没有可提取的记忆"));
        assert!(is_explicit_no_memory_response("当前对话没有值得提取的内容"));
    }

    #[test]
    fn test_no_memory_negative() {
        assert!(!is_explicit_no_memory_response(""));
        assert!(!is_explicit_no_memory_response("用户喜欢喝咖啡"));
        assert!(!is_explicit_no_memory_response("T1: 一些重要记忆内容"));
    }

    #[test]
    fn test_no_memory_trim() {
        assert!(is_explicit_no_memory_response("  暂无记忆  "));
    }

    #[test]
    fn test_rate_limit_detection() {
        assert!(is_rate_limit_error("HTTP 429 Too Many Requests"));
        assert!(is_rate_limit_error("rate limit exceeded"));
        assert!(is_rate_limit_error("rate-limited"));
    }

    #[test]
    fn test_rate_limit_negative() {
        assert!(!is_rate_limit_error(""));
        assert!(!is_rate_limit_error("connection timeout"));
        assert!(!is_rate_limit_error("internal server error"));
    }

    #[test]
    fn test_parse_empty() {
        let result = parse_extraction_response("");
        assert!(result.is_empty());

        let result = parse_extraction_response("   ");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_pipe_format_single() {
        let input = "用户喜欢喝咖啡|T1|T1|preference|开心|0.8|偏好|coffee,drink";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "用户喜欢喝咖啡");
        assert_eq!(result[0].source_turn_start, 1);
        assert_eq!(result[0].source_turn_end, 1);
        assert_eq!(result[0].memory_category, "preference");
        assert_eq!(result[0].emotional_tone, "开心");
        assert_eq!(result[0].importance, 4);
        assert_eq!(result[0].fact_kind, "偏好");
    }

    #[test]
    fn test_parse_pipe_format_multi() {
        let input =
            "用户喜欢咖啡|T1|T1|preference|喜欢|0.7||\n用户住在北京|T2|T2|fact|平静|0.9|背景|";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "用户喜欢咖啡");
        assert_eq!(result[1].content, "用户住在北京");
    }

    #[test]
    fn test_parse_pipe_format_with_directives() {
        let input = "[TONE:开心]\n用户喜欢咖啡|T1|T1|preference||0.7||";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].emotional_tone, "开心");
    }

    #[test]
    fn test_parse_pipe_format_invalid_anchor_skipped() {
        let input = "content|TX|TY|cat||0.5||";
        let result = parse_extraction_response(input);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_pipe_format_short_content_skipped() {
        let input = "ab|T1|T1|cat||0.5||";
        let result = parse_extraction_response(input);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_tagged_format_important() {
        let input = "[IMPORTANT][T1-T3] 用户说他喜欢喝咖啡";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "用户说他喜欢喝咖啡");
        assert_eq!(result[0].source_turn_start, 1);
        assert_eq!(result[0].source_turn_end, 3);
        assert!(result[0].is_important);
        assert_eq!(result[0].importance, 5);
        assert_eq!(result[0].memory_category, "important");
    }

    #[test]
    fn test_parse_tagged_format_normal() {
        let input = "[NORMAL:3][T2] 用户今天心情不错";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "用户今天心情不错");
        assert_eq!(result[0].source_turn_start, 2);
        assert_eq!(result[0].source_turn_end, 2);
        assert!(!result[0].is_important);
        assert_eq!(result[0].importance, 3);
    }

    #[test]
    fn test_parse_tagged_format_with_user_prefix() {
        let input = "[IMPORTANT][T1] 用户张三: 我喜欢编程";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "我喜欢编程");
    }

    #[test]
    fn test_parse_tagged_format_with_category_directive() {
        let input = "[CATEGORY:核心事实]\n[IMPORTANT][T1] 用户是一名软件工程师";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].memory_category, "core_fact");
    }

    #[test]
    fn test_parse_tagged_format_with_fact_kind() {
        let input = "[FACT_KIND:背景]\n[NORMAL:4][T2] 用户毕业于清华大学";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "用户毕业于清华大学");
        assert_eq!(result[0].fact_kind, "背景");
    }

    #[test]
    fn test_parse_no_memory_skipped() {
        let input = "暂无记忆";
        let result = parse_extraction_response(input);
        assert!(result.is_empty());

        let input2 = "no memory found in this conversation";
        let result2 = parse_extraction_response(input2);
        assert!(result2.is_empty());
    }

    #[test]
    fn test_parse_comment_lines_skipped() {
        let input = "# 这是一条注释\n用户喜欢咖啡|T1|T1|fact|平静|0.5||";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "用户喜欢咖啡");
    }

    #[test]
    fn test_parse_empty_lines_skipped() {
        let input = "\n\n用户喜欢咖啡|T1|T1|fact|平静|0.5||\n\n";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_parse_tone_directive_invalid_skipped() {
        let input = "[TONE:invalid_tone]\n用户喜欢咖啡|T1|T1|fact||0.5||";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].emotional_tone, "");
    }

    #[test]
    fn test_parse_strip_memory_prefix() {
        let input = "普通记忆：用户的信息|T1|T1|fact||0.5||";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert!(result[0].content.contains("用户的信息"));
    }

    #[test]
    fn test_parse_reflection_with_conflict() {
        let input = r#"{"has_conflict": true, "conflict_type": "update", "action": "update_old", "summary": "位置信息变更", "reason": "用户从上海搬到北京", "confidence": 0.85}"#;
        let result = parse_reflection_response(input);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.has_conflict);
        assert_eq!(r.conflict_type, "update");
        assert_eq!(r.action, "update_old");
        assert_eq!(r.summary, "位置信息变更");
        assert_eq!(r.reason, "用户从上海搬到北京");
        assert!((r.confidence - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_parse_reflection_no_conflict() {
        let input = r#"{"has_conflict": false, "conflict_type": "none", "action": "keep_both", "confidence": 0.0}"#;
        let result = parse_reflection_response(input);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(!r.has_conflict);
        assert_eq!(r.conflict_type, "none");
        assert_eq!(r.action, "keep_both");
    }

    #[test]
    fn test_parse_reflection_json_in_text() {
        let input = "这里是一些分析文本\n```json\n{\"has_conflict\": true, \"conflict_type\": \"contradiction\", \"action\": \"replace_old\"}\n```\n以上就是分析结果";
        let result = parse_reflection_response(input);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.has_conflict);
        assert_eq!(r.conflict_type, "contradiction");
    }

    #[test]
    fn test_parse_reflection_empty() {
        assert!(parse_reflection_response("").is_none());
        assert!(parse_reflection_response("no json here").is_none());
    }

    #[test]
    fn test_parse_reflection_defaults() {
        let input = r#"{"has_conflict": true}"#;
        let result = parse_reflection_response(input);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.has_conflict);
        assert_eq!(r.conflict_type, "none");
        assert_eq!(r.action, "keep_both");
    }

    #[test]
    fn test_parse_reflection_confidence_clamped() {
        let input = r#"{"has_conflict": true, "confidence": 1.5}"#;
        let result = parse_reflection_response(input);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!((r.confidence - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_extract_json_object_bare() {
        let input = r#"{"key": "value"}"#;
        let result = extract_json_object(input);
        assert_eq!(result, r#"{"key": "value"}"#);
    }

    #[test]
    fn test_extract_json_object_in_text() {
        let input = "prefix text {\"key\": \"value\"} suffix text";
        let result = extract_json_object(input);
        assert_eq!(result, r#"{"key": "value"}"#);
    }

    #[test]
    fn test_extract_json_object_empty() {
        assert_eq!(extract_json_object(""), "");
        assert_eq!(extract_json_object("no braces"), "");
    }
}
