use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::MemoryItem;
use crate::prelude::XueliResult;
use crate::traits::ai_client::AIClient;
use crate::traits::prompt_template::PromptTemplateLoader;
use super::extractor::MemoryExtractor;
use super::models::{ExtractionConfig, MemoryReflectionResult};
use super::reflection::MemoryReflection;

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

    pub async fn call_llm_for_reflection(
        &self,
        existing: &[MemoryItem],
        new_items: &[MemoryItem],
    ) -> XueliResult<MemoryReflectionResult> {
        if let Some(ref reflection) = self.reflection {
            let result = reflection.reflect(existing, new_items).await?;
            Ok(MemoryReflectionResult {
                has_conflict: result.has_conflict,
                conflict_type: result.conflict_type,
                action: result.action,
                summary: result.summary,
                reason: result.reason,
                confidence: result.confidence,
                evidence: result.evidence,
                targets: result.targets,
            })
        } else {
            Ok(MemoryReflectionResult::default())
        }
    }

    pub fn is_suppressed_memory(metadata: &serde_json::Value) -> bool {
        metadata
            .get("patch_status")
            .and_then(|v| v.as_str())
            .map(|s| s == "superseded" || s == "contextualized")
            .unwrap_or(false)
    }

    const EXISTING_RECORDS_LIMIT: usize = 30;

    pub fn format_existing_records_section(
        records: &[HashMap<String, serde_json::Value>],
    ) -> String {
        if records.is_empty() {
            return String::new();
        }
        let mut lines: Vec<String> = Vec::new();
        for (index, record) in records.iter().enumerate().take(Self::EXISTING_RECORDS_LIMIT) {
            let kind = record
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("ordinary");
            let kind_label = if kind == "important" { "重要" } else { "普通" };
            let content = record
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if content.is_empty() {
                continue;
            }
            lines.push(format!("{}. [{}] {}", index + 1, kind_label, content));
        }
        if lines.is_empty() {
            return String::new();
        }
        let header = "【已有记忆】".to_string();
        let suffix = if records.len() > Self::EXISTING_RECORDS_LIMIT {
            format!(
                "\n（仅展示前 {} 条，共 {} 条）",
                Self::EXISTING_RECORDS_LIMIT,
                records.len()
            )
        } else {
            String::new()
        };
        format!("{}\n{}{}", header, lines.join("\n"), suffix)
    }

    pub fn build_extraction_prompt_content(
        user_id: &str,
        dialogue_text: &str,
        narrative_summary: &str,
        existing_records_section: &str,
    ) -> String {
        let narrative_context = if !narrative_summary.is_empty() {
            format!(
                "当前对话主线摘要：\n- {}\n如果本轮提取与主线冲突，请以当前对话明确表达为准。",
                narrative_summary
            )
        } else {
            String::new()
        };

        let mut user_content = format!("【用户】\n用户ID={}", user_id);
        if !narrative_context.is_empty() {
            user_content = format!("{}\n{}", user_content, narrative_context);
        }
        user_content = format!("{}\n\n【对话记录】\n{}", user_content, dialogue_text);
        if !existing_records_section.is_empty() {
            user_content = format!("{}\n\n{}", user_content, existing_records_section);
        }
        user_content
    }
}
