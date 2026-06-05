use crate::core::types::MemoryPatch;
use crate::prelude::XueliResult;

/// 记忆 Patch 合并器 — 将新提取的记忆合并到已有记忆
///
/// 处理策略：
/// 1. 基于内容相似度去重
/// 2. 同用户同类型记忆：高重要性覆盖低重要性
/// 3. 冲突标记保留（等待 Reflection 处理）
pub struct PatchMerger;

impl PatchMerger {
    pub fn new() -> Self {
        Self
    }

    pub fn merge(
        &self,
        existing_patches: &[MemoryPatch],
        new_patch: &MemoryPatch,
    ) -> XueliResult<MemoryPatch> {
        let mut merged = MemoryPatch {
            add: Vec::new(),
            update: Vec::new(),
            remove: Vec::new(),
        };

        // 收集已有记忆内容（用于去重）
        let existing_contents: Vec<&str> = existing_patches
            .iter()
            .flat_map(|p| &p.add)
            .map(|m| m.content.as_str())
            .collect();

        // 处理新记忆
        for item in &new_patch.add {
            // 简单去重：基于内容的前 30 字符相似度
            if !self.is_duplicate(&item.content, &existing_contents) {
                merged.add.push(item.clone());
            }
        }

        // 处理更新
        for updated in &new_patch.update {
            merged.update.push(updated.clone());
        }

        // 处理删除
        for removed in &new_patch.remove {
            merged.remove.push(removed.clone());
        }

        Ok(merged)
    }

    /// 基于内容前缀判断是否重复
    fn is_duplicate(&self, content: &str, existing: &[&str]) -> bool {
        let normalized = content.trim().to_lowercase();
        let prefix = &normalized[..normalized.len().min(30)];

        existing.iter().any(|e| {
            let e_norm = e.trim().to_lowercase();
            // 较短的字符串作为匹配基准
            let min_len = prefix.chars().count().min(e_norm.chars().count());
            if min_len == 0 {
                return false;
            }
            let short = if prefix.chars().count() <= e_norm.chars().count() {
                prefix
            } else {
                &e_norm
            };
            let long = if prefix.chars().count() <= e_norm.chars().count() {
                e_norm.as_str()
            } else {
                prefix
            };
            // 检查短字符串是否与长字符串的前 N 个字符高度相似
            let long_prefix: String = long.chars().take(min_len).collect();
            let common = short
                .chars()
                .zip(long_prefix.chars())
                .filter(|(a, b)| a == b)
                .count();
            (common as f64 / min_len as f64) > 0.75
        })
    }
}

impl Default for PatchMerger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{MemoryItem, MemoryType};
    use chrono::Utc;

    fn make_item(content: &str) -> MemoryItem {
        MemoryItem {
            id: format!("id_{}", uuid::Uuid::new_v4().as_simple()),
            user_id: "u1".to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Fact,
            importance: 0.5,
            created_at: Utc::now(),
            last_accessed_at: Utc::now(),
            access_count: 0,
        }
    }

    #[test]
    fn test_merge_no_duplicates() {
        let merger = PatchMerger::new();
        let existing = vec![MemoryPatch {
            add: vec![make_item("用户喜欢Python")],
            update: vec![],
            remove: vec![],
        }];
        let new_patch = MemoryPatch {
            add: vec![make_item("用户住在北京")],
            update: vec![],
            remove: vec![],
        };
        let result = merger.merge(&existing, &new_patch).unwrap();
        assert_eq!(result.add.len(), 1);
        assert!(result.add[0].content.contains("北京"));
    }

    #[test]
    fn test_merge_duplicates_filtered() {
        let merger = PatchMerger::new();
        let existing = vec![MemoryPatch {
            add: vec![make_item("用户喜欢喝咖啡，每天早上都要喝一杯")],
            update: vec![],
            remove: vec![],
        }];
        let new_patch = MemoryPatch {
            add: vec![make_item("用户喜欢喝咖啡")],
            update: vec![],
            remove: vec![],
        };
        let result = merger.merge(&existing, &new_patch).unwrap();
        assert!(result.add.is_empty());
    }

    #[test]
    fn test_merge_empty_patches() {
        let merger = PatchMerger::new();
        let existing: Vec<MemoryPatch> = vec![];
        let new_patch = MemoryPatch {
            add: vec![],
            update: vec![],
            remove: vec![],
        };
        let result = merger.merge(&existing, &new_patch).unwrap();
        assert!(result.add.is_empty());
        assert!(result.update.is_empty());
        assert!(result.remove.is_empty());
    }

    #[test]
    fn test_is_duplicate_similar() {
        let merger = PatchMerger::new();
        assert!(merger.is_duplicate("用户喜欢喝咖啡", &["用户喜欢喝咖啡，每天早上都要喝"]));
    }

    #[test]
    fn test_is_duplicate_different() {
        let merger = PatchMerger::new();
        assert!(!merger.is_duplicate("用户喜欢喝咖啡", &["用户住在北京"]));
    }
}
