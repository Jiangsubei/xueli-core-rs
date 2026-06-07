use std::collections::HashMap;

use crate::core::scope::ChatScope;
use crate::core::types::{MemoryItem, MemoryType};

/// 记忆元数据的可见性和分类标注
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryVisibility {
    Private,
    Shared,
}

/// 记忆内容的语义分类
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryContentCategory {
    PersonalInfo,
    Preference,
    Relationship,
    Event,
    Opinion,
    Skill,
    Health,
    Finance,
    DailyChat,
    Generic,
}

/// 记忆的适用范围
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryApplicabilityScope {
    SelfOnly,
    DirectUsers,
    GroupMembers,
    Public,
    Unknown,
}

/// 记忆访问策略 — 决定哪些记忆对当前上下文可见。
///
/// 包含三层过滤：类型过滤、隐私可见性、适用范围。
pub struct MemoryAccessPolicy {
    pub private_allowed_types: Vec<MemoryType>,
    pub group_allowed_types: Vec<MemoryType>,
}

/// 提示词条目类型
pub type PromptEntry = HashMap<String, serde_json::Value>;

impl MemoryAccessPolicy {
    pub fn new() -> Self {
        Self {
            private_allowed_types: vec![
                MemoryType::Fact,
                MemoryType::Preference,
                MemoryType::Event,
                MemoryType::Opinion,
                MemoryType::Relationship,
            ],
            group_allowed_types: vec![MemoryType::Fact, MemoryType::Event],
        }
    }

    /// 判断某条记忆在当前作用域下是否可访问
    pub fn is_accessible(&self, memory: &MemoryItem, scope: &ChatScope) -> bool {
        let allowed = match scope {
            ChatScope::Private => &self.private_allowed_types,
            ChatScope::Group(_) => &self.group_allowed_types,
        };
        allowed.contains(&memory.memory_type)
    }

    /// 过滤可访问的记忆
    pub fn filter_accessible(&self, memories: &[MemoryItem], scope: &ChatScope) -> Vec<MemoryItem> {
        memories
            .iter()
            .filter(|m| self.is_accessible(m, scope))
            .cloned()
            .collect()
    }

    /// 去重：按内容文本相同去除重复记忆，保留 importance 最高的
    pub fn dedupe_entries(memories: &[MemoryItem]) -> Vec<MemoryItem> {
        let mut best: HashMap<String, MemoryItem> = HashMap::new();
        for m in memories {
            let key = m.content.trim().to_lowercase();
            best.entry(key)
                .and_modify(|existing| {
                    if m.importance > existing.importance {
                        *existing = m.clone();
                    }
                })
                .or_insert_with(|| m.clone());
        }
        let mut result: Vec<_> = best.into_values().collect();
        result.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        result
    }

    /// 降序排列（按 importance）
    pub fn sort_by_importance(memories: &mut [MemoryItem]) {
        memories.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// 检查元数据是否标记为 shared
    pub fn is_shared(&self, metadata: Option<&serde_json::Value>) -> bool {
        let meta = match metadata {
            Some(v) => v,
            None => return false,
        };
        meta.get("visibility")
            .and_then(|v| v.as_str())
            .map(|s| s == "shared")
            .unwrap_or(false)
    }

    /// 检查元数据是否标记为 addressing preference
    pub fn is_addressing(&self, metadata: Option<&serde_json::Value>) -> bool {
        let meta = match metadata {
            Some(v) => v,
            None => return false,
        };
        meta.get("content_category")
            .and_then(|v| v.as_str())
            .map(|s| s == "addressing_preference")
            .unwrap_or(false)
    }

    /// 为提示词分类记忆：返回 "private"、"shared" 或 "addressing"
    pub fn classify_for_prompt(
        &self,
        metadata: Option<&serde_json::Value>,
        owner_user_id: &str,
        requester_user_id: &str,
    ) -> &'static str {
        if self.is_addressing(metadata) {
            return "addressing";
        }
        if self.is_shared(metadata) && owner_user_id != requester_user_id {
            return "shared";
        }
        "private"
    }

    /// 对提示词条目去重（按规范化内容文本）
    pub fn dedupe_prompt_entries(&self, entries: &[PromptEntry]) -> Vec<PromptEntry> {
        let mut seen: HashMap<String, PromptEntry> = HashMap::new();
        for entry in entries {
            let content = entry
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if content.is_empty() || seen.contains_key(&content) {
                continue;
            }
            seen.insert(content, entry.clone());
        }
        seen.into_values().collect()
    }
}

impl Default for MemoryAccessPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_item(content: &str, importance: f64, mt: MemoryType) -> MemoryItem {
        MemoryItem {
            id: format!("id_{}", content),
            user_id: "u1".into(),
            content: content.into(),
            memory_type: mt,
            importance,
            created_at: Utc::now(),
            last_accessed_at: Utc::now(),
            access_count: 0,
        }
    }

    #[test]
    fn test_filter_accessible_group_blocks_opinion() {
        let policy = MemoryAccessPolicy::new();
        let items = vec![
            make_item("事实A", 0.8, MemoryType::Fact),
            make_item("观点B", 0.7, MemoryType::Opinion),
        ];
        let filtered = policy.filter_accessible(&items, &ChatScope::Group("g1".into()));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].content, "事实A");
    }

    #[test]
    fn test_filter_accessible_private_allows_all() {
        let policy = MemoryAccessPolicy::new();
        let items = vec![
            make_item("事实A", 0.8, MemoryType::Fact),
            make_item("观点B", 0.7, MemoryType::Opinion),
            make_item("关系C", 0.6, MemoryType::Relationship),
        ];
        let filtered = policy.filter_accessible(&items, &ChatScope::Private);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn test_dedupe_entries() {
        let items = vec![
            make_item("一样的内容", 0.5, MemoryType::Fact),
            make_item("一样的内容", 0.9, MemoryType::Fact),
        ];
        let deduped = MemoryAccessPolicy::dedupe_entries(&items);
        assert_eq!(deduped.len(), 1);
        assert!((deduped[0].importance - 0.9).abs() < 0.001);
    }
}
