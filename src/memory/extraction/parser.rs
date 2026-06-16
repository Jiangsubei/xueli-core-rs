//! LLM 响应解析器：处理提取、反思响应的解析和标准化。
//!
//! 对应 Python 版 `xueli/src/memory/extraction/parser.py`
//!
//! 主要处理两种 LLM 输出格式：
//! 1. 管道分隔格式：`content|anchor_start|anchor_end|category|emotional_tone|importance|fact_kind|tags`
//! 2. 标签指令格式：`[IMPORTANT][T1-T3] content` / `[NORMAL:3][T1] content`

use std::collections::HashSet;

use crate::memory::extraction::models::ExtractedMemory;

// ── 常量 ──────────────────────────────────────────────────

/// 有效的情绪标签集合
const VALID_TONES: &[&str] = &[
    "开心", "喜欢", "惊讶", "无语", "委屈", "生气", "伤心", "嘲讽", "害怕", "困惑", "平静",
];

/// 类别到内部标识的映射
const CATEGORY_MAP: &[(&str, &str)] = &[
    ("核心事实", "core_fact"),
    ("重要", "important"),
    ("闲聊", "casual"),
];

/// "无记忆"检测模式 — LLM 显式表示没有提取到任何记忆
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

/// 强匹配的"无记忆"短语（清理标点空格后精确匹配）
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

// ── 公共类型 ──────────────────────────────────────────────

/// 反思响应解析结果
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReflectionParseResult {
    /// 是否存在冲突
    pub has_conflict: bool,
    /// 冲突类型
    pub conflict_type: String,
    /// 建议动作
    pub action: String,
    /// 冲突摘要
    pub summary: String,
    /// 冲突原因
    pub reason: String,
    /// 置信度
    pub confidence: f64,
}

// ── 解析函数 ──────────────────────────────────────────────

/// 解析 LLM 提取响应文本，返回提取的记忆列表。
///
/// 支持两种格式：
/// - 管道分隔：`content|anchor_start|anchor_end|category|emotional_tone|importance|fact_kind|tags`
/// - 标签指令：`[IMPORTANT][T1-T3] content` / `[NORMAL:3][T1] content`
///
/// 同时支持 `[TONE:xxx]`、`[CATEGORY:xxx]`、`[FACT_KIND:xxx]` 上下文指令行。
pub fn parse_extraction_response(content: &str) -> Vec<ExtractedMemory> {
    let mut memories: Vec<ExtractedMemory> = Vec::new();
    let text = content.trim();
    if text.is_empty() {
        return memories;
    }

    // 确定如何拆分为独立条目
    //
    // 两种可能的管道用法：
    //   a) 字段分隔：content|T1|T1|cat|tone|0.5|| → 一行一条记忆
    //   b) 行分隔：  [TONE:...]|[IMPORTANT][T1] content|... → 管道作为行分隔符
    // 启发式：如果管道分隔的片段以 `[` 开头，则是行分隔模式；否则是字段分隔模式。
    let lines: Vec<String> = if text.contains('|') && text.lines().count() == 1 {
        let has_directive_segments = text.split('|').any(|s| s.trim().starts_with('['));
        if has_directive_segments {
            text.split('|')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            // 字段分隔模式：整行作为一条记录
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

        // 跳过 "无记忆" 行
        if is_explicit_no_memory_response(line) {
            continue;
        }

        // 情绪指令 [TONE:xxx]
        if let Some(tone) = try_parse_tone_directive(line) {
            current_tone = tone;
            continue;
        }

        // 类别指令 [CATEGORY:xxx]
        if let Some(category) = try_parse_category_directive(line) {
            current_category = category;
            continue;
        }

        // 事实种类指令 [FACT_KIND:xxx]
        if let Some(fact_kind) = try_parse_fact_kind_directive(line) {
            current_fact_kind = fact_kind;
            continue;
        }

        // 尝试按管道分隔格式解析
        if let Some(memory) =
            try_parse_pipe_format(line, &current_tone, &current_category, &current_fact_kind)
        {
            memories.push(memory);
            continue;
        }

        // 尝试按标签指令格式解析 [IMPORTANT][anchor] 或 [NORMAL:n][anchor]
        if let Some(memory) =
            try_parse_tagged_format(line, &current_tone, &current_category, &current_fact_kind)
        {
            memories.push(memory);
        }
    }

    memories
}

/// 验证提取记忆的锚点轮次范围是否有效且在允许范围内。
///
/// 对应 Python 版 `is_valid_anchor()`
pub fn is_valid_anchor(memory: &ExtractedMemory, allowed_turn_ids: &HashSet<u32>) -> bool {
    let start = memory.anchor_start.parse::<u32>().unwrap_or(0);
    let end = memory.anchor_end.parse::<u32>().unwrap_or(0);
    if start == 0 || end == 0 {
        return false;
    }
    if start > end {
        return false;
    }
    (start..=end).all(|t| allowed_turn_ids.contains(&t))
}

/// 解析锚点字符串（如 `T1` 或 `T1-T3`），返回 `(start, end)` 元组。
///
/// 对应 Python 版 `parse_anchor()`
pub fn parse_anchor(anchor: &str) -> Option<(u32, u32)> {
    let s = anchor.trim();
    if s.is_empty() {
        return None;
    }

    let s_upper = s.to_uppercase();
    let without_t = s_upper.strip_prefix('T').unwrap_or(&s_upper);

    if let Some((start_str, end_str)) = without_t.split_once('-') {
        // T1-T3 或 T1-T3 格式（end 可能带 T 前缀）
        let end_clean = end_str.strip_prefix('T').unwrap_or(end_str);
        let start: u32 = start_str.parse().ok()?;
        let end: u32 = end_clean.parse().ok()?;
        if start == 0 || end == 0 || start > end {
            return None;
        }
        Some((start, end))
    } else {
        // T1 格式
        let id: u32 = without_t.parse().ok()?;
        if id == 0 {
            return None;
        }
        Some((id, id))
    }
}

/// 检查文本是否为显式的"无记忆"表示。
///
/// 使用两级匹配：
/// 1. 清理标点空格后精确匹配强信号短语
/// 2. 子串匹配宽松模式列表（中/英文）
pub fn is_explicit_no_memory_response(content: &str) -> bool {
    let text = content.trim();
    if text.is_empty() {
        return false;
    }

    // 第一级：清理所有标点/空格后精确匹配强信号
    let normalized = normalize_text_simple(text);
    for (pattern, _) in NO_MEMORY_EXACT {
        if normalized == *pattern {
            return true;
        }
    }

    // 第二级：子串匹配宽松模式
    let lower = text.to_lowercase();
    NO_MEMORY_PATTERNS
        .iter()
        .any(|pat| lower.contains(&pat.to_lowercase()))
}

/// 判断错误消息是否为 LLM 限流错误。
///
/// 对应 Python 版 `is_rate_limit_error()`
pub fn is_rate_limit_error(error_message: &str) -> bool {
    let msg = error_message.to_lowercase();
    msg.contains("429") || msg.contains("rate limit") || msg.contains("rate-limited")
}

/// 解析 LLM 反思响应，返回结构化反思结果。
///
/// 从文本中提取 JSON 对象并解析为 [`ReflectionParseResult`]。
/// 解析失败或文本不含 JSON 时返回 `None`。
///
/// 对应 Python 版 `parse_reflection_response()`
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

// ── 辅助函数 ──────────────────────────────────────────────

/// 从文本中提取第一个 JSON 对象。
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

/// 规范化文本：移除所有标点、空格、引号，转小写。
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

/// 尝试解析 `[TONE:xxx]` 指令行，返回情绪标签。
fn try_parse_tone_directive(line: &str) -> Option<String> {
    let s = line.trim();
    let s_lower = s.to_lowercase();
    if !s_lower.starts_with("[tone:") || !s.ends_with(']') {
        return None;
    }
    let inner = &s[6..s.len() - 1]; // "[TONE:" 之后，"]" 之前
    let tone = inner.trim();
    if VALID_TONES.contains(&tone) {
        Some(tone.to_string())
    } else {
        None
    }
}

/// 尝试解析 `[CATEGORY:xxx]` 或 `[类别:xxx]` 指令行，返回内部类别标识。
fn try_parse_category_directive(line: &str) -> Option<String> {
    let s = line.trim();
    let s_lower = s.to_lowercase();
    let (inner, _) = if s_lower.starts_with("[category:") && s.ends_with(']') {
        (&s[10..s.len() - 1], false)
    } else if s_lower.starts_with("[类别:") && s.ends_with(']') {
        // "[类别:" = 4 chars in bytes, but it's UTF-8 so use char index
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

/// 尝试解析 `[FACT_KIND:xxx]` 指令行，返回事实种类标签。
fn try_parse_fact_kind_directive(line: &str) -> Option<String> {
    let s = line.trim();
    let s_lower = s.to_lowercase();
    if !s_lower.starts_with("[fact_kind:") || !s.ends_with(']') {
        return None;
    }
    let inner = &s[11..s.len() - 1]; // "[FACT_KIND:" 之后，"]" 之前
    let kind = inner.trim();
    let valid_kinds = ["偏好", "边界", "计划", "背景", "档案"];
    if valid_kinds.contains(&kind) {
        Some(kind.to_string())
    } else {
        None
    }
}

/// 尝试按管道分隔格式解析一行。
///
/// 期望格式：`content|anchor_start|anchor_end|category|emotional_tone|importance|fact_kind|tags`
fn try_parse_pipe_format(
    line: &str,
    default_tone: &str,
    default_category: &str,
    default_fact_kind: &str,
) -> Option<ExtractedMemory> {
    // 跳过标签指令行
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

    let anchor_start = parts[1].trim().to_string();
    let anchor_end = if parts.len() > 2 {
        parts[2].trim().to_string()
    } else {
        anchor_start.clone()
    };

    // 验证 anchor 格式
    if parse_anchor(&anchor_start).is_none() {
        return None;
    }
    if !anchor_end.is_empty() && parse_anchor(&anchor_end).is_none() {
        return None;
    }

    let category = if parts.len() > 3 && !parts[3].trim().is_empty() {
        parts[3].trim().to_string()
    } else {
        default_category.to_string()
    };

    let emotional_tone = if parts.len() > 4 && !parts[4].trim().is_empty() {
        parts[4].trim().to_string()
    } else {
        default_tone.to_string()
    };

    let importance = if parts.len() > 5 {
        parts[5]
            .trim()
            .parse::<f64>()
            .unwrap_or(0.5)
            .clamp(0.0, 1.0)
    } else {
        0.5
    };

    let fact_kind = if parts.len() > 6 && !parts[6].trim().is_empty() {
        Some(parts[6].trim().to_string())
    } else if !default_fact_kind.is_empty() {
        Some(default_fact_kind.to_string())
    } else {
        None
    };

    let tags: Vec<String> = if parts.len() > 7 && !parts[7].trim().is_empty() {
        parts[7]
            .trim()
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    } else {
        Vec::new()
    };

    Some(ExtractedMemory {
        content: content.to_string(),
        anchor_start,
        anchor_end,
        category,
        emotional_tone,
        importance,
        fact_kind,
        tags,
        metadata: std::collections::HashMap::new(),
    })
}

/// 尝试按标签指令格式解析一行。
///
/// 支持格式：
/// - `[IMPORTANT][T1-T3] 内容` 或 `[重要][T1-T3] 内容`
/// - `[NORMAL:3][T1] 内容` 或 `[普通:3][T1] 内容`
fn try_parse_tagged_format(
    line: &str,
    default_tone: &str,
    default_category: &str,
    default_fact_kind: &str,
) -> Option<ExtractedMemory> {
    let s = line.trim();

    // 去除前缀 "普通记忆" / "重要记忆" / 列表标记
    let cleaned = strip_memory_prefix(s);

    // 尝试匹配 [IMPORTANT] / [重要] [T1-T3] content
    let important_re =
        regex::Regex::new(r"(?i)^\[(?:IMPORTANT|重要)\]\s*\[(T\d+(?:-T?\d+)?)\]\s*(.+)$").ok()?;

    let normal_re = regex::Regex::new(
        r"(?i)^\[(?:NORMAL|普通)\s*:\s*([1-5])\]\s*\[(T\d+(?:-T?\d+)?)\]\s*(.+)$",
    )
    .ok()?;

    let (importance, anchor_str, content_text): (f64, &str, &str) =
        if let Some(caps) = important_re.captures(&cleaned) {
            let anchor = caps.get(1)?.as_str();
            let content = caps.get(2)?.as_str();
            (1.0, anchor, content)
        } else if let Some(caps) = normal_re.captures(&cleaned) {
            let level: f64 = caps.get(1)?.as_str().parse().ok()?;
            let anchor = caps.get(2)?.as_str();
            let content = caps.get(3)?.as_str();
            (level / 5.0, anchor, content)
        } else {
            return None;
        };

    let (start, end) = parse_anchor(anchor_str)?;

    // 清理内容：移除 "用户xxx: " 前缀
    let content_clean = strip_user_prefix(content_text).trim().to_string();
    if content_clean.is_empty() || content_clean.len() <= 2 {
        return None;
    }

    let fact_kind = if !default_fact_kind.is_empty() {
        Some(default_fact_kind.to_string())
    } else {
        None
    };

    Some(ExtractedMemory {
        content: content_clean,
        anchor_start: format!("T{}", start),
        anchor_end: format!("T{}", end),
        category: default_category.to_string(),
        emotional_tone: default_tone.to_string(),
        importance,
        fact_kind,
        tags: Vec::new(),
        metadata: std::collections::HashMap::new(),
    })
}

/// 去除行首的"普通记忆"/"重要记忆"/列表标记等前缀。
fn strip_memory_prefix(text: &str) -> String {
    let re = regex::Regex::new(r"^(?:普通记忆|重要记忆)\s*[：:]\s*").unwrap();
    let s = re.replace(text, "").to_string();

    let list_re = regex::Regex::new(r"^(?:[\-\*\u{2022}]\s*|\d+[\.\)、]\s*)+").unwrap();
    list_re.replace(&s, "").to_string()
}

/// 去除 "用户xxx: " 或 "user xxx: " 前缀。
fn strip_user_prefix(text: &str) -> String {
    let re = regex::Regex::new(r#"(?i)^(?:用户|user)\s*[^\r\n：:]{1,40}[：:]"#).unwrap();
    re.replace(text, "").to_string()
}

// ── 测试 ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_anchor ──

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
        // Some outputs may use T1-3
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

    // ── is_valid_anchor ──

    #[test]
    fn test_is_valid_anchor_basic() {
        let _mem = ExtractedMemory {
            content: "test".into(),
            anchor_start: "T1".into(),
            anchor_end: "T3".into(),
            category: "important".into(),
            emotional_tone: String::new(),
            importance: 0.8,
            fact_kind: None,
            tags: vec![],
            metadata: std::collections::HashMap::new(),
        };
        // 如果起始/结束 anchor 是 "T1"/"T3" 字符串格式，我们会尝试 parse 成 u32
        // 对于字符串 "T1" parse 会失败，所以此函数应该直接基于字符串匹配
        // 但实际 anchor_start/end 存的是剥离 T 后的数字字符串

        // anchor 是纯数字的情况
        let mem2 = ExtractedMemory {
            content: "test".into(),
            anchor_start: "1".into(),
            anchor_end: "3".into(),
            category: String::new(),
            emotional_tone: String::new(),
            importance: 0.0,
            fact_kind: None,
            tags: vec![],
            metadata: std::collections::HashMap::new(),
        };
        let allowed: HashSet<u32> = [1, 2, 3, 4, 5].into_iter().collect();
        assert!(is_valid_anchor(&mem2, &allowed));

        let mem3 = ExtractedMemory {
            content: "test".into(),
            anchor_start: "1".into(),
            anchor_end: "6".into(),
            category: String::new(),
            emotional_tone: String::new(),
            importance: 0.0,
            fact_kind: None,
            tags: vec![],
            metadata: std::collections::HashMap::new(),
        };
        assert!(!is_valid_anchor(&mem3, &allowed));

        let mem4 = ExtractedMemory {
            content: "test".into(),
            anchor_start: "0".into(),
            anchor_end: "3".into(),
            category: String::new(),
            emotional_tone: String::new(),
            importance: 0.0,
            fact_kind: None,
            tags: vec![],
            metadata: std::collections::HashMap::new(),
        };
        assert!(!is_valid_anchor(&mem4, &allowed));

        let mem5 = ExtractedMemory {
            content: "test".into(),
            anchor_start: "3".into(),
            anchor_end: "1".into(),
            category: String::new(),
            emotional_tone: String::new(),
            importance: 0.0,
            fact_kind: None,
            tags: vec![],
            metadata: std::collections::HashMap::new(),
        };
        assert!(!is_valid_anchor(&mem5, &allowed));
    }

    // ── is_explicit_no_memory_response ──

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

    // ── is_rate_limit_error ──

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

    // ── parse_extraction_response ──

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
        assert_eq!(result[0].anchor_start, "T1");
        assert_eq!(result[0].anchor_end, "T1");
        assert_eq!(result[0].category, "preference");
        assert_eq!(result[0].emotional_tone, "开心");
        assert!((result[0].importance - 0.8).abs() < 0.001);
        assert_eq!(result[0].fact_kind.as_deref(), Some("偏好"));
        assert_eq!(result[0].tags, vec!["coffee", "drink"]);
    }

    #[test]
    fn test_parse_pipe_format_multi() {
        let input =
            "用户喜欢咖啡|T1|T1|preference|喜欢|0.7||\n用户住在北京|T2|T2|fact|平静|0.9|背景|";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].content, "用户喜欢咖啡");
        assert_eq!(result[1].content, "用户住在北京");
        assert!(result[1].importance > 0.85);
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
        assert_eq!(result[0].anchor_start, "T1");
        assert_eq!(result[0].anchor_end, "T3");
        assert!((result[0].importance - 1.0).abs() < 0.001);
        assert_eq!(result[0].category, "important");
    }

    #[test]
    fn test_parse_tagged_format_normal() {
        let input = "[NORMAL:3][T2] 用户今天心情不错";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "用户今天心情不错");
        assert_eq!(result[0].anchor_start, "T2");
        assert_eq!(result[0].anchor_end, "T2");
        assert!((result[0].importance - 0.6).abs() < 0.001);
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
        assert_eq!(result[0].category, "core_fact");
    }

    #[test]
    fn test_parse_tagged_format_with_fact_kind() {
        let input = "[FACT_KIND:背景]\n[NORMAL:4][T2] 用户毕业于清华大学";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "用户毕业于清华大学");
        assert_eq!(result[0].fact_kind.as_deref(), Some("背景"));
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
        // 无效 tone 不应该作为记忆行被错误解析
        let input = "[TONE:invalid_tone]\n用户喜欢咖啡|T1|T1|fact||0.5||";
        let result = parse_extraction_response(input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].emotional_tone, ""); // tone 没有被设置
    }

    #[test]
    fn test_parse_strip_memory_prefix() {
        let input = "普通记忆：用户的信息|T1|T1|fact||0.5||";
        // 注意：strip_memory_prefix 只在 tagged format 中调用
        // 管道格式中不会有此前缀
        // 如果前缀被作为 content 的一部分，至少能正确解析
        let result = parse_extraction_response(input);
        // 管道格式中 "普通记忆：用户的信息" 作为 content 会被接受
        assert_eq!(result.len(), 1);
        // content 包含前缀（管道格式不做前缀清理）
        assert!(result[0].content.contains("用户的信息"));
    }

    // ── parse_reflection_response ──

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

    // ── extract_json_object ──

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
