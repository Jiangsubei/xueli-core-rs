use crate::core::types::{MemoryItem, MemoryType};
use crate::core::scope::ChatScope;

/// 记忆访问策略 — 决定哪些记忆对当前上下文可见
pub struct MemoryAccessPolicy {
    /// 私聊中允许访问的记忆类型
    pub private_allowed_types: Vec<MemoryType>,
    /// 群聊中允许访问的记忆类型
    pub group_allowed_types: Vec<MemoryType>,
}

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
            group_allowed_types: vec![
                MemoryType::Fact,
                MemoryType::Event,
            ],
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
    pub fn filter_accessible(
        &self,
        memories: &[MemoryItem],
        scope: &ChatScope,
    ) -> Vec<MemoryItem> {
        memories
            .iter()
            .filter(|m| self.is_accessible(m, scope))
            .cloned()
            .collect()
    }
}

impl Default for MemoryAccessPolicy {
    fn default() -> Self {
        Self::new()
    }
}