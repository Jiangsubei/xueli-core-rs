use crate::core::types::MemoryPatch;

/// 记忆 Patch 合并器 — 将新提取的记忆合并到已有记忆
pub struct PatchMerger;

impl PatchMerger {
    pub fn new() -> Self {
        Self
    }

    pub fn merge(
        &self,
        _existing_patches: &[MemoryPatch],
        _new_patch: &MemoryPatch,
    ) -> Result<MemoryPatch, String> {
        // TODO: 实现 Patch 去重与合并逻辑
        Ok(MemoryPatch {
            add: Vec::new(),
            update: Vec::new(),
            remove: Vec::new(),
        })
    }
}

impl Default for PatchMerger {
    fn default() -> Self {
        Self::new()
    }
}