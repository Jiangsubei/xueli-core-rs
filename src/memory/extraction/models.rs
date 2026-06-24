use std::collections::HashMap;

/// Extracted memory from conversation
#[derive(Debug, Clone)]
pub struct ExtractedMemory {
    pub content: String,
    pub source_turn_start: u32,
    pub source_turn_end: u32,
    pub is_important: bool,
    pub importance: u32,
    pub emotional_tone: String,
    pub memory_category: String,
    pub fact_kind: String,
}

/// Extraction configuration
#[derive(Debug, Clone)]
pub struct ExtractionConfig {
    pub extract_every_n_turns: usize,
    pub max_dialogue_length: usize,
    pub min_memory_quality: f64,
    pub reflection_enabled: bool,
    pub reflection_candidate_limit: usize,
    pub reflection_min_topic_overlap: f64,
    pub prompt_template_name: String,
    pub system_prompt: String,
    pub max_retries: u32,
    pub temperature: f64,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            extract_every_n_turns: 3,
            max_dialogue_length: 10,
            min_memory_quality: 0.7,
            reflection_enabled: true,
            reflection_candidate_limit: 3,
            reflection_min_topic_overlap: 0.45,
            prompt_template_name: "memory_extraction.prompt".to_string(),
            system_prompt: String::new(),
            max_retries: 3,
            temperature: 0.3,
        }
    }
}

/// Memory reflection result
#[derive(Debug, Clone)]
pub struct MemoryReflectionResult {
    pub has_conflict: bool,
    pub conflict_type: String,
    pub action: String,
    pub summary: String,
    pub reason: String,
    pub confidence: f64,
    pub evidence: Vec<HashMap<String, serde_json::Value>>,
    pub targets: Vec<HashMap<String, serde_json::Value>>,
}

impl Default for MemoryReflectionResult {
    fn default() -> Self {
        Self {
            has_conflict: false,
            conflict_type: "none".to_string(),
            action: "keep_both".to_string(),
            summary: String::new(),
            reason: String::new(),
            confidence: 0.0,
            evidence: Vec::new(),
            targets: Vec::new(),
        }
    }
}
