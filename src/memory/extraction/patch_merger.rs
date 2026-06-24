use crate::core::types::{MemoryItem, MemoryPatch};
use crate::memory::memory_dispute_resolver::ReflectionPayload;
use crate::memory::stores::important::ImportantMemoryStore;
use crate::memory::stores::memory_item::SqliteMemoryItemStore;
use crate::prelude::XueliResult;
use serde_json::Value as JsonValue;

/// 记忆 Patch 合并器 — 将新提取的记忆合并到已有记忆
///
/// 处理策略：
/// 1. 基于内容相似度去重
/// 2. 同用户同类型记忆：高重要性覆盖低重要性
/// 3. 冲突标记保留（等待 Reflection 处理）
/// 4. 基于反射结果更新记忆 patch 元数据
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

    /// Check if metadata shows the memory was suppressed
    pub fn is_suppressed_memory(metadata: &serde_json::Value) -> bool {
        metadata
            .get("patch_status")
            .and_then(|v| v.as_str())
            .map(|s| s == "superseded" || s == "contextualized")
            .unwrap_or(false)
    }

    /// Map reflection action to new memory patch_status
    pub fn resolve_new_memory_patch_status(action: &str) -> String {
        match action.to_lowercase().as_str() {
            "prefer_existing" => "superseded".to_string(),
            "prefer_new" | "keep_both_prefer_recent" | "merge_context" => {
                "active_patch".to_string()
            }
            _ => "conflict_reflected".to_string(),
        }
    }

    /// Map reflection action to existing memory patch_status
    pub fn resolve_existing_memory_patch_status(action: &str) -> String {
        match action.to_lowercase().as_str() {
            "prefer_new" | "keep_both_prefer_recent" => "superseded".to_string(),
            "merge_context" => "contextualized".to_string(),
            _ => "active".to_string(),
        }
    }

    /// Check if ordinary memory should be promoted to important
    /// Aligned with Python: checks metadata dict for memory_type, importance, mention_count
    pub fn should_promote_to_important(metadata: &serde_json::Value) -> bool {
        let memory_type = metadata
            .get("memory_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        if memory_type != "ordinary" {
            return false;
        }
        let importance = metadata
            .get("importance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if importance < 5.0 {
            return false;
        }
        let mention_count = metadata
            .get("mention_count")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as u32;
        mention_count >= 2
    }

    /// 基于反射结果更新记忆上的 patch 元数据
    ///
    /// 根据 `ReflectionPayload` 中的 action 决定如何处理新旧记忆之间的关系：
    /// - `prefer_existing` → 新记忆标记为 superseded，旧记忆保持 active
    /// - `prefer_new` → 新记忆标记为 active_patch，旧记忆标记为 superseded
    /// - `keep_both_prefer_recent` → 新记忆 active_patch，旧记忆 superseded
    /// - `merge_context` → 新记忆 active_patch，旧记忆 contextualized
    pub async fn apply_patch_merge(
        store: &SqliteMemoryItemStore,
        new_memory: &MemoryItem,
        existing_memory: &MemoryItem,
        reflection: &ReflectionPayload,
    ) -> XueliResult<()> {
        let new_patch_status = Self::resolve_new_memory_patch_status(&reflection.action);
        let existing_patch_status = Self::resolve_existing_memory_patch_status(&reflection.action);

        Self::update_patch_metadata(store, &new_memory.id, "patch_status", &new_patch_status)
            .await?;
        Self::update_patch_metadata(
            store,
            &new_memory.id,
            "patch_relation",
            &format!("reflected_against_{}", existing_memory.id),
        )
        .await?;

        Self::update_patch_metadata(
            store,
            &existing_memory.id,
            "patch_status",
            &existing_patch_status,
        )
        .await?;
        Self::update_patch_metadata(
            store,
            &existing_memory.id,
            "patch_relation",
            &format!("reflected_against_{}", new_memory.id),
        )
        .await?;

        // 如果新记忆被采纳，需要将旧记忆抑制写入普通存储
        if reflection.action == "prefer_new" || reflection.action == "keep_both_prefer_recent" {
            Self::apply_to_ordinary(store, existing_memory, &existing_patch_status).await?;
        }
        if reflection.action == "prefer_existing" {
            Self::apply_to_ordinary(store, new_memory, &new_patch_status).await?;
        }

        Ok(())
    }

    /// 更新单条记忆的 patch 元数据字段
    async fn update_patch_metadata(
        store: &SqliteMemoryItemStore,
        mem_id: &str,
        key: &str,
        value: &str,
    ) -> XueliResult<()> {
        store.update_metadata(mem_id, key, value).await
    }

    /// 将 patch 状态写入普通存储（通过 replace_user_memories 或 update_metadata）
    async fn apply_to_ordinary(
        store: &SqliteMemoryItemStore,
        memory: &MemoryItem,
        patch_status: &str,
    ) -> XueliResult<()> {
        // 更新该记忆的元数据，标记其 patch 状态
        let mut meta = match store.load_metadata(&memory.user_id, &memory.id).await? {
            Some(m) => m,
            None => JsonValue::Object(Default::default()),
        };
        if let JsonValue::Object(ref mut map) = meta {
            map.insert(
                "patch_status".to_string(),
                JsonValue::String(patch_status.to_string()),
            );
            map.insert(
                "patch_resolved_at".to_string(),
                JsonValue::String(chrono::Utc::now().to_rfc3339()),
            );
        }
        store
            .update_metadata_full(&memory.user_id, &memory.id, &meta)
            .await?;
        Ok(())
    }

    /// 将 patch 状态写入重要记忆存储
    pub async fn apply_to_important(
        store: &ImportantMemoryStore,
        memory: &MemoryItem,
        patch_status: &str,
    ) -> XueliResult<()> {
        let meta_json = serde_json::json!({
            "patch_status": patch_status,
            "patch_resolved_at": chrono::Utc::now().to_rfc3339(),
        });
        store
            .update_metadata_json(&memory.id, &serde_json::to_string(&meta_json).unwrap_or_default())
            .await?;
        Ok(())
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

    #[test]
    fn test_is_suppressed_memory_superseded() {
        let metadata = serde_json::json!({"patch_status": "superseded"});
        assert!(PatchMerger::is_suppressed_memory(&metadata));
    }

    #[test]
    fn test_is_suppressed_memory_contextualized() {
        let metadata = serde_json::json!({"patch_status": "contextualized"});
        assert!(PatchMerger::is_suppressed_memory(&metadata));
    }

    #[test]
    fn test_is_suppressed_memory_active() {
        let metadata = serde_json::json!({"patch_status": "active"});
        assert!(!PatchMerger::is_suppressed_memory(&metadata));
    }

    #[test]
    fn test_is_suppressed_memory_no_status() {
        let metadata = serde_json::json!({});
        assert!(!PatchMerger::is_suppressed_memory(&metadata));
    }

    #[test]
    fn test_resolve_new_memory_patch_status_prefer_existing() {
        assert_eq!(
            PatchMerger::resolve_new_memory_patch_status("prefer_existing"),
            "superseded"
        );
    }

    #[test]
    fn test_resolve_new_memory_patch_status_prefer_new() {
        assert_eq!(
            PatchMerger::resolve_new_memory_patch_status("prefer_new"),
            "active_patch"
        );
    }

    #[test]
    fn test_resolve_new_memory_patch_status_keep_both() {
        assert_eq!(
            PatchMerger::resolve_new_memory_patch_status("keep_both_prefer_recent"),
            "active_patch"
        );
    }

    #[test]
    fn test_resolve_new_memory_patch_status_merge_context() {
        assert_eq!(
            PatchMerger::resolve_new_memory_patch_status("merge_context"),
            "active_patch"
        );
    }

    #[test]
    fn test_resolve_new_memory_patch_status_unknown() {
        assert_eq!(
            PatchMerger::resolve_new_memory_patch_status("unknown_action"),
            "conflict_reflected"
        );
    }

    #[test]
    fn test_resolve_existing_memory_patch_status_prefer_new() {
        assert_eq!(
            PatchMerger::resolve_existing_memory_patch_status("prefer_new"),
            "superseded"
        );
    }

    #[test]
    fn test_resolve_existing_memory_patch_status_keep_both() {
        assert_eq!(
            PatchMerger::resolve_existing_memory_patch_status("keep_both_prefer_recent"),
            "superseded"
        );
    }

    #[test]
    fn test_resolve_existing_memory_patch_status_merge_context() {
        assert_eq!(
            PatchMerger::resolve_existing_memory_patch_status("merge_context"),
            "contextualized"
        );
    }

    #[test]
    fn test_resolve_existing_memory_patch_status_unknown() {
        assert_eq!(
            PatchMerger::resolve_existing_memory_patch_status("prefer_existing"),
            "active"
        );
    }

    #[test]
    fn test_should_promote_to_important_eligible() {
        let metadata = serde_json::json!({
            "memory_type": "ordinary",
            "importance": 5,
            "mention_count": 3
        });
        assert!(PatchMerger::should_promote_to_important(&metadata));
    }

    #[test]
    fn test_should_promote_to_important_not_eligible_low_importance() {
        let metadata = serde_json::json!({
            "memory_type": "ordinary",
            "importance": 4,
            "mention_count": 3
        });
        assert!(!PatchMerger::should_promote_to_important(&metadata));
    }

    #[test]
    fn test_should_promote_to_important_not_eligible_low_access() {
        let metadata = serde_json::json!({
            "memory_type": "ordinary",
            "importance": 5,
            "mention_count": 1
        });
        assert!(!PatchMerger::should_promote_to_important(&metadata));
    }

    #[test]
    fn test_should_promote_to_important_not_ordinary() {
        let metadata = serde_json::json!({
            "memory_type": "important",
            "importance": 5,
            "mention_count": 3
        });
        assert!(!PatchMerger::should_promote_to_important(&metadata));
    }
}
