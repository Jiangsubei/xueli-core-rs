// Reply 流水线编排 — 多层级记忆上下文加载与格式化
// 对应 Python 版 xueli/src/handlers/reply/pipeline.py
//
// 注意：完整的流水线编排依赖 PromptPlan 的策略字段（memory_profile 等），
// Rust 版简化了这部分逻辑，当前实现聚焦于记忆上下文加载与格式化。

use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

use crate::core::config::XueliConfig;
use crate::memory::manager::MemoryManager;
use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};

use crate::traits::prompt_template::PromptTemplateLoader;

/// 多层级记忆上下文加载结果
#[derive(Debug, Clone, Default)]
pub struct MemoryContextResult {
    /// 人物事实上下文
    pub person_fact_context: String,
    /// 持久记忆上下文
    pub persistent_memory_context: String,
    /// 会话恢复上下文
    pub session_restore_context: String,
    /// 精确回忆上下文
    pub precise_recall_context: String,
    /// 动态记忆上下文
    pub dynamic_memory_context: String,
    /// 近期对话历史消息
    pub history_messages: Vec<ConversationRecord>,
    /// 是否首轮对话
    pub is_first_turn: bool,
    /// 复用的视觉分析描述（从近期消息中提取）
    pub reusable_vision_analysis: Option<String>,
    /// 角色卡提示词条目
    pub character_card_entries: Vec<HashMap<String, serde_json::Value>>,
    /// 叙事线摘要
    pub narrative_thread_summary: Option<String>,
}

// ── 模块级工具函数（不依赖泛型参数 L）────────────────────

/// 格式化记忆行列表为编号文本
pub fn format_memory_context(memory_lines: &[String]) -> String {
    if memory_lines.is_empty() {
        return String::new();
    }
    memory_lines
        .iter()
        .enumerate()
        .map(|(i, text)| format!("{}. {}", i + 1, text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 格式化并去重记忆行
pub fn format_memory_context_and_dedupe(
    memory_lines: &[String],
    existing_context: &str,
) -> String {
    let existing = collect_memory_lines_from_text(existing_context);
    let deduped = dedupe_memory_lines(memory_lines, &existing);
    format_memory_context(&deduped)
}

/// 从已格式化的记忆上下文文本中提取原始行
pub fn collect_memory_lines_from_text(memory_context: &str) -> Vec<String> {
    memory_context
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if let Some(pos) = trimmed.find(". ") {
                if trimmed[..pos].chars().all(|c| c.is_ascii_digit()) {
                    return trimmed[pos + 2..].to_string();
                }
            }
            trimmed.to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// 记忆行去重（基于规范化后的文本）
pub fn dedupe_memory_lines(memory_lines: &[String], existing_lines: &[String]) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = existing_lines
        .iter()
        .map(|line| normalize_memory_line(line))
        .filter(|n| !n.is_empty())
        .collect();

    let mut deduped: Vec<String> = Vec::new();
    for line in memory_lines {
        let normalized = normalize_memory_line(line);
        if normalized.is_empty() || seen.contains(&normalized) {
            continue;
        }
        seen.insert(normalized);
        deduped.push(line.clone());
    }
    deduped
}

/// 规范化记忆行（去空白、小写）
fn normalize_memory_line(line: &str) -> String {
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Reply 流水线 — 负责在 ReplyAgent 执行前加载并格式化所有记忆上下文
pub struct ReplyPipeline<L: PromptTemplateLoader + 'static> {
    #[allow(dead_code)]
    config: Arc<XueliConfig>,
    memory_manager: Option<Arc<MemoryManager<L>>>,
    conversation_store: Option<Arc<SqliteConversationStore>>,
}

impl<L: PromptTemplateLoader + 'static> ReplyPipeline<L> {
    pub fn new(
        config: Arc<XueliConfig>,
        memory_manager: Option<Arc<MemoryManager<L>>>,
        conversation_store: Option<Arc<SqliteConversationStore>>,
    ) -> Self {
        Self {
            config,
            memory_manager,
            conversation_store,
        }
    }

    /// 加载多层级记忆上下文
    pub async fn load_memory_context(
        &self,
        user_id: &str,
        scope_id: &str,
        is_group: bool,
        existing_message_count: usize,
    ) -> MemoryContextResult {
        let is_first_turn = existing_message_count == 0;
        let scope_type = if is_group { "group" } else { "private" };

        let mut result = MemoryContextResult {
            is_first_turn,
            ..Default::default()
        };

        // 层级1：人物事实
        if let Some(ref mm) = self.memory_manager {
            match mm.get_by_user(user_id).await {
                Ok(items) => {
                    let facts: Vec<String> = items
                        .iter()
                        .filter(|item| {
                            matches!(
                                item.memory_type,
                                crate::core::types::MemoryType::Fact
                                    | crate::core::types::MemoryType::Preference
                                    | crate::core::types::MemoryType::Relationship
                            )
                        })
                        .take(6)
                        .map(|item| item.content.clone())
                        .collect();
                    if !facts.is_empty() {
                        result.person_fact_context = facts.join("\n");
                    }
                }
                Err(e) => {
                    warn!("[回复管道] 加载人物事实失败: {}", e);
                }
            }

            // 层级2：持久记忆（取重要度高的）
            match mm.get_by_user(user_id).await {
                Ok(items) => {
                    let mut important: Vec<_> = items
                        .iter()
                        .filter(|item| item.importance > 0.5)
                        .cloned()
                        .collect();
                    important.sort_by(|a, b| {
                        b.importance
                            .partial_cmp(&a.importance)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    let lines: Vec<String> = important
                        .iter()
                        .take(5)
                        .map(|item| item.content.clone())
                        .collect();
                    if !lines.is_empty() {
                        result.persistent_memory_context = format_memory_context(&lines);
                    }
                }
                Err(e) => {
                    warn!("[回复管道] 加载持久记忆失败: {}", e);
                }
            }

            // 层级3/4/5：session_restore / precise_recall / dynamic 记忆
            self.load_retrieval_context(mm, user_id, scope_id, scope_type, &mut result)
                .await;
        }

        // 历史消息：从 conversation store 加载
        if let Some(ref store) = self.conversation_store {
            match store.get_recent_by_scope(scope_type, scope_id, 30) {
                Ok(records) => {
                    result.history_messages = records;
                    if result.history_messages.is_empty() {
                        result.is_first_turn = true;
                    }
                }
                Err(e) => {
                    warn!("[回复管道] 加载对话历史失败: {}", e);
                }
            }
        }

        result
    }

    async fn load_retrieval_context(
        &self,
        mm: &Arc<MemoryManager<L>>,
        user_id: &str,
        _scope_id: &str,
        _scope_type: &str,
        result: &mut MemoryContextResult,
    ) {
        match mm.build_prompt_context(user_id, "").await {
            Ok(ctx_result) => {
                if !ctx_result.session_restore.is_empty() {
                    let entries: Vec<String> = ctx_result
                        .session_restore
                        .iter()
                        .filter_map(|e| {
                            e.get("content")
                                .and_then(|v| v.as_str().map(|s| s.to_string()))
                        })
                        .collect();
                    result.session_restore_context =
                        format_memory_context_and_dedupe(&entries, "");
                }
                if !ctx_result.dynamic_memories.is_empty() {
                    let entries: Vec<String> = ctx_result
                        .dynamic_memories
                        .iter()
                        .filter_map(|e| {
                            e.get("content")
                                .and_then(|v| v.as_str().map(|s| s.to_string()))
                        })
                        .collect();
                    result.dynamic_memory_context = format_memory_context_and_dedupe(
                        &entries,
                        &result.persistent_memory_context,
                    );
                }
                if !ctx_result.precise_recall.is_empty() {
                    let entries: Vec<String> = ctx_result
                        .precise_recall
                        .iter()
                        .filter_map(|e| {
                            e.get("content")
                                .and_then(|v| v.as_str().map(|s| s.to_string()))
                        })
                        .collect();
                    result.precise_recall_context = format_memory_context(&entries);
                }
            }
            Err(e) => {
                warn!("[回复管道] 检索上下文失败: {}", e);
            }
        }
    }

    /// 设置会话恢复上下文（由外部注入，如 SessionRestoreService）
    pub fn inject_session_restore_context(result: &mut MemoryContextResult, entries: &[String]) {
        result.session_restore_context = format_memory_context(entries);
    }

    /// 设置动态记忆上下文（由外部注入）
    pub fn inject_dynamic_memory_context(result: &mut MemoryContextResult, memories: &[String]) {
        result.dynamic_memory_context =
            format_memory_context_and_dedupe(memories, &result.persistent_memory_context);
    }

    /// 注入角色卡提示词条目
    pub fn inject_character_card_entries(
        result: &mut MemoryContextResult,
        entries: Vec<HashMap<String, serde_json::Value>>,
    ) {
        result.character_card_entries = entries;
    }

    /// 注入叙事线摘要
    pub fn inject_narrative_thread_summary(result: &mut MemoryContextResult, summary: String) {
        result.narrative_thread_summary = Some(summary);
    }

    /// 从历史图片描述列表中提取可复用的视觉分析
    pub fn extract_reusable_vision_analysis(image_descriptions: &[String]) -> Option<String> {
        let non_empty: Vec<&str> = image_descriptions
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !s.trim().is_empty() && !s.contains("未成功识别"))
            .collect();
        if non_empty.is_empty() {
            None
        } else if non_empty.len() == 1 {
            Some(format!("[图片] {}", non_empty[0]))
        } else {
            Some(format!(
                "[图片] {}",
                non_empty
                    .iter()
                    .enumerate()
                    .map(|(i, desc)| format!("图{}: {}", i + 1, desc))
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        }
    }

    /// 构建对话历史文本 — 格式化带身份标签的消息流
    pub fn build_conversation_history_text(
        records: &[ConversationRecord],
        assistant_name: &str,
    ) -> String {
        let mut lines: Vec<String> = Vec::with_capacity(records.len());
        for rec in records {
            let label = if rec.is_bot {
                assistant_name.to_string()
            } else if rec.sender_name.is_empty() {
                "用户".to_string()
            } else {
                format!("用户{}", rec.sender_name)
            };
            lines.push(format!("{}: {}", label, rec.text));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_memory_context() {
        let lines = vec!["记忆1".into(), "记忆2".into()];
        let result = format_memory_context(&lines);
        assert_eq!(result, "1. 记忆1\n2. 记忆2");
    }

    #[test]
    fn test_format_memory_context_empty() {
        let result = format_memory_context(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_collect_memory_lines_from_text() {
        let text = "1. 第一行\n2. 第二行\n3. 第三行";
        let lines = collect_memory_lines_from_text(text);
        assert_eq!(lines, vec!["第一行", "第二行", "第三行"]);
    }

    #[test]
    fn test_dedupe_memory_lines() {
        let new = vec!["记忆A".into(), "记忆B".into(), "记忆C".into()];
        let existing = vec!["记忆A".into(), "记忆D".into()];
        let result = dedupe_memory_lines(&new, &existing);
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"记忆B".to_string()));
        assert!(result.contains(&"记忆C".to_string()));
        assert!(!result.contains(&"记忆A".to_string()));
    }

    #[test]
    fn test_normalize_memory_line() {
        assert_eq!(normalize_memory_line("  多   余  空白  "), "多 余 空白");
        assert_eq!(normalize_memory_line("Hello World"), "hello world");
    }

    #[test]
    fn test_format_and_dedupe() {
        let existing = "1. 已有记忆";
        let new = vec!["已有记忆".into(), "新记忆".into()];
        let result = format_memory_context_and_dedupe(&new, existing);
        assert_eq!(result, "1. 新记忆");
    }

    #[test]
    fn test_extract_reusable_vision_analysis_single() {
        let descs = vec!["图片显示一只猫".to_string()];
        let result = ReplyPipeline::<crate::services::prompt_loader::NoopPromptTemplateLoader>::extract_reusable_vision_analysis(&descs);
        assert!(result.is_some());
        assert!(result.unwrap().contains("猫"));
    }

    #[test]
    fn test_extract_reusable_vision_analysis_empty() {
        let result = ReplyPipeline::<crate::services::prompt_loader::NoopPromptTemplateLoader>::extract_reusable_vision_analysis(&[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_build_conversation_history_text() {
        use crate::memory::stores::conversation::ConversationRecord;
        let records = vec![
            ConversationRecord {
                id: 1,
                session_id: "s1".into(),
                user_id: "u1".into(),
                sender_name: "小明".into(),
                text: "你好".into(),
                is_bot: false,
                scope_type: "group".into(),
                scope_id: "g1".into(),
                event_time: 100.0,
                message_id: "m1".into(),
                platform: "qq".into(),
            },
            ConversationRecord {
                id: 2,
                session_id: "s1".into(),
                user_id: "u1".into(),
                sender_name: "bot".into(),
                text: "你好呀！".into(),
                is_bot: true,
                scope_type: "group".into(),
                scope_id: "g1".into(),
                event_time: 101.0,
                message_id: "".into(),
                platform: "".into(),
            },
        ];
        let text = ReplyPipeline::<crate::services::prompt_loader::NoopPromptTemplateLoader>::build_conversation_history_text(&records, "小丽");
        assert!(text.contains("用户小明: 你好"));
        assert!(text.contains("小丽: 你好呀！"));
    }

    #[tokio::test]
    async fn test_load_memory_context_empty() {
        let config = Arc::new(XueliConfig::default());
        let pipeline = ReplyPipeline::<crate::services::prompt_loader::NoopPromptTemplateLoader>::new(config, None, None);
        let result = pipeline
            .load_memory_context("user1", "scope1", false, 0)
            .await;
        assert!(result.is_first_turn);
        assert!(result.person_fact_context.is_empty());
        assert!(result.persistent_memory_context.is_empty());
    }
}
