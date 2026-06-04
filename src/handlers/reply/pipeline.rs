// Reply 流水线编排 — 多层级记忆上下文加载与格式化
// 对应 Python 版 xueli/src/handlers/reply/pipeline.py
//
// 注意：完整的流水线编排依赖 PromptPlan 的策略字段（memory_profile 等），
// Rust 版简化了这部分逻辑，当前实现聚焦于记忆上下文加载与格式化。

use std::sync::Arc;
use tracing::warn;

use crate::core::config::XueliConfig;
use crate::memory::manager::MemoryManager;
use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};

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
}

/// Reply 流水线 — 负责在 ReplyAgent 执行前加载并格式化所有记忆上下文
pub struct ReplyPipeline {
    config: Arc<XueliConfig>,
    memory_manager: Option<Arc<MemoryManager>>,
    conversation_store: Option<Arc<SqliteConversationStore>>,
}

impl ReplyPipeline {
    pub fn new(
        config: Arc<XueliConfig>,
        memory_manager: Option<Arc<MemoryManager>>,
        conversation_store: Option<Arc<SqliteConversationStore>>,
    ) -> Self {
        Self {
            config,
            memory_manager,
            conversation_store,
        }
    }

    /// 加载多层级记忆上下文
    ///
    /// 返回：
    /// - person_fact_context: 人物事实
    /// - persistent_memory_context: 持久记忆
    /// - session_restore_context: 会话恢复摘要
    /// - precise_recall_context: 精确回忆
    /// - dynamic_memory_context: 动态记忆
    /// - history_messages: 近期对话历史
    /// - is_first_turn: 是否首轮
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
                        result.persistent_memory_context = Self::format_memory_context(&lines);
                    }
                }
                Err(e) => {
                    warn!("[回复管道] 加载持久记忆失败: {}", e);
                }
            }
        }

        // 层级3/4/5 以及历史：从 conversation store 加载
        if let Some(ref store) = self.conversation_store {
            match store.get_recent_by_scope(scope_type, scope_id, 30) {
                Ok(records) => {
                    result.history_messages = records;
                    result.is_first_turn = result.history_messages.is_empty();
                }
                Err(e) => {
                    warn!("[回复管道] 加载对话历史失败: {}", e);
                }
            }
        }

        // 会话恢复上下文通过 SessionRestoreService 提供（外部注入）
        // precise_recall / dynamic_memory 通过 MemoryManager 的 BM25/向量检索提供（外部注入）
        // 此处留空，由调用方按需填充

        result
    }

    /// 设置会话恢复上下文（由外部注入，如 SessionRestoreService）
    pub fn inject_session_restore_context(result: &mut MemoryContextResult, entries: &[String]) {
        result.session_restore_context = Self::format_memory_context(entries);
    }

    /// 设置动态记忆上下文（由外部注入）
    pub fn inject_dynamic_memory_context(result: &mut MemoryContextResult, memories: &[String]) {
        result.dynamic_memory_context =
            Self::format_memory_context_and_dedupe(memories, &result.persistent_memory_context);
    }

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
        let existing: Vec<String> = Self::collect_memory_lines_from_text(existing_context);
        let deduped = Self::dedupe_memory_lines(memory_lines, &existing);
        Self::format_memory_context(&deduped)
    }

    /// 从已格式化的记忆上下文文本中提取原始行
    pub fn collect_memory_lines_from_text(memory_context: &str) -> Vec<String> {
        memory_context
            .lines()
            .map(|line| {
                // 去掉行首编号（如 "1. xxx" → "xxx"）
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
            .map(|line| Self::normalize_memory_line(line))
            .filter(|n| !n.is_empty())
            .collect();

        let mut deduped: Vec<String> = Vec::new();
        for line in memory_lines {
            let normalized = Self::normalize_memory_line(line);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_memory_context() {
        let lines = vec!["记忆1".into(), "记忆2".into()];
        let result = ReplyPipeline::format_memory_context(&lines);
        assert_eq!(result, "1. 记忆1\n2. 记忆2");
    }

    #[test]
    fn test_format_memory_context_empty() {
        let result = ReplyPipeline::format_memory_context(&[]);
        assert_eq!(result, "");
    }

    #[test]
    fn test_collect_memory_lines_from_text() {
        let text = "1. 第一行\n2. 第二行\n3. 第三行";
        let lines = ReplyPipeline::collect_memory_lines_from_text(text);
        assert_eq!(lines, vec!["第一行", "第二行", "第三行"]);
    }

    #[test]
    fn test_dedupe_memory_lines() {
        let new = vec!["记忆A".into(), "记忆B".into(), "记忆C".into()];
        let existing = vec!["记忆A".into(), "记忆D".into()];
        let result = ReplyPipeline::dedupe_memory_lines(&new, &existing);
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"记忆B".to_string()));
        assert!(result.contains(&"记忆C".to_string()));
        assert!(!result.contains(&"记忆A".to_string()));
    }

    #[test]
    fn test_normalize_memory_line() {
        assert_eq!(
            ReplyPipeline::normalize_memory_line("  多   余  空白  "),
            "多 余 空白"
        );
        assert_eq!(
            ReplyPipeline::normalize_memory_line("Hello World"),
            "hello world"
        );
    }

    #[test]
    fn test_format_and_dedupe() {
        let existing = "1. 已有记忆";
        let new = vec!["已有记忆".into(), "新记忆".into()];
        let result = ReplyPipeline::format_memory_context_and_dedupe(&new, existing);
        assert_eq!(result, "1. 新记忆");
    }

    #[tokio::test]
    async fn test_load_memory_context_empty() {
        let config = Arc::new(XueliConfig::default());
        let pipeline = ReplyPipeline::new(config, None, None);
        let result = pipeline
            .load_memory_context("user1", "scope1", false, 0)
            .await;
        assert!(result.is_first_turn);
        assert!(result.person_fact_context.is_empty());
        assert!(result.persistent_memory_context.is_empty());
    }
}
