use std::collections::HashMap;

/// Extracted memory from conversation
#[derive(Debug, Clone)]
pub struct ExtractedMemory {
    pub content: String,
    pub anchor_start: String, // T1-T3 format
    pub anchor_end: String,
    pub category: String,
    pub emotional_tone: String,
    pub importance: f64, // 0.0-1.0
    pub fact_kind: Option<String>,
    pub tags: Vec<String>,
    pub metadata: HashMap<String, String>,
}

/// Extraction configuration
#[derive(Debug, Clone)]
pub struct ExtractionConfig {
    pub max_retries: u32,
    pub temperature: f64,
    pub min_memory_quality: f64,
    pub max_dialogue_length: usize,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            temperature: 0.3,
            min_memory_quality: 0.4,
            max_dialogue_length: 30,
        }
    }
}

/// Memory reflection result
#[derive(Debug, Clone, Default)]
pub struct MemoryReflectionResult {
    pub conflicts: Vec<MemoryConflict>,
    pub patterns: Vec<String>,
    pub suggestions: Vec<String>,
    pub confidence: f64,
}

/// Memory conflict
#[derive(Debug, Clone)]
pub struct MemoryConflict {
    pub old_memory: String,
    pub new_memory: String,
    pub conflict_type: String,
    pub resolution: String,
    pub reason: String,
}
