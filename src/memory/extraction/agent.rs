use std::sync::Arc;

use crate::core::types::MemoryItem;
use crate::prelude::XueliResult;
use crate::traits::ai_client::AIClient;
use crate::traits::prompt_template::PromptTemplateLoader;
use super::extractor::MemoryExtractor;
use super::models::{ExtractionConfig, MemoryReflectionResult};
use super::reflection::MemoryReflection;

/// Extraction agent that coordinates LLM extraction and reflection.
/// Acts as a facade over MemoryExtractor and MemoryReflection.
pub struct ExtractionAgent<A: AIClient, L: PromptTemplateLoader> {
    extractor: Arc<MemoryExtractor<A, L>>,
    reflection: Option<Arc<MemoryReflection<A, L>>>,
    config: ExtractionConfig,
}

impl<A: AIClient, L: PromptTemplateLoader> ExtractionAgent<A, L> {
    pub fn new(
        extractor: Arc<MemoryExtractor<A, L>>,
        reflection: Option<Arc<MemoryReflection<A, L>>>,
        config: ExtractionConfig,
    ) -> Self {
        Self {
            extractor,
            reflection,
            config,
        }
    }

    /// Call LLM for memory extraction with retries
    pub async fn call_llm_for_extraction(
        &self,
        user_id: &str,
        messages: &[String],
    ) -> XueliResult<Vec<MemoryItem>> {
        self.extractor
            .extract(user_id, messages)
            .await
            .map(|patch| patch.add)
    }

    /// Call LLM for reflection
    pub async fn call_llm_for_reflection(
        &self,
        existing: &[MemoryItem],
        new_items: &[MemoryItem],
    ) -> XueliResult<MemoryReflectionResult> {
        if let Some(ref reflection) = self.reflection {
            let result = reflection.reflect(existing, new_items).await?;
            Ok(MemoryReflectionResult {
                conflicts: Vec::new(),
                patterns: result.resolutions.clone(),
                suggestions: result.resolutions,
                confidence: result.confidence,
            })
        } else {
            Ok(MemoryReflectionResult::default())
        }
    }

    /// Check if a memory is suppressed
    pub fn is_suppressed_memory(&self, content: &str) -> bool {
        let text = content.trim().to_lowercase();
        if text.is_empty() {
            return false;
        }
        // Check for suppression patterns
        let suppression_patterns = [
            "不知道", "不确定", "可能", "也许", "好像",
            "not sure", "maybe", "perhaps", "unclear",
        ];
        suppression_patterns.iter().any(|p| text.contains(p))
    }
}