use std::collections::HashSet;
use std::sync::Arc;

use crate::core::types::MemoryItem;
use crate::memory::stores::important::{ImportantMemory, ImportantMemoryStore};
use crate::memory::stores::memory_item::SqliteMemoryItemStore;
use crate::memory::stores::person_fact::{PersonFact, SqlitePersonFactStore};
use crate::prelude::XueliResult;

/// 人物事实服务 — 从重要记忆同步并生成人物事实
pub struct PersonFactService {
    fact_store: Arc<SqlitePersonFactStore>,
    important_store: Arc<ImportantMemoryStore>,
    #[allow(dead_code)]
    memory_store: Arc<SqliteMemoryItemStore>,
    prompt_limit: usize,
}

impl PersonFactService {
    pub fn new(
        fact_store: Arc<SqlitePersonFactStore>,
        important_store: Arc<ImportantMemoryStore>,
        memory_store: Arc<SqliteMemoryItemStore>,
    ) -> Self {
        Self {
            fact_store,
            important_store,
            memory_store,
            prompt_limit: 6,
        }
    }

    /// 设置 prompt 中事实的最大数量
    pub fn with_prompt_limit(mut self, limit: usize) -> Self {
        self.prompt_limit = limit;
        self
    }

    /// 同步用户的重要记忆到人物事实
    pub async fn sync_user_facts(&self, user_id: &str) -> XueliResult<Vec<PersonFact>> {
        let important_memories = self.important_store.get_memories(user_id, 1).await?;

        let generated = self.build_facts_from_important_memories(user_id, &important_memories);

        let existing = self.fact_store.get_by_user(user_id).await?;

        if facts_equal(&existing, &generated) {
            return Ok(existing);
        }

        for fact in &generated {
            self.fact_store.store(fact.clone()).await?;
        }

        let generated_ids: HashSet<String> = generated.iter().map(|f| f.id.clone()).collect();
        for old in &existing {
            if !generated_ids.contains(&old.id) {
                self.fact_store.delete(&old.id).await?;
            }
        }

        Ok(generated)
    }

    /// 获取用于 prompt 的事实条目
    pub async fn get_prompt_entries(
        &self,
        user_id: &str,
        limit: Option<usize>,
    ) -> XueliResult<Vec<PersonFactEntry>> {
        let facts = self.sync_user_facts(user_id).await?;
        let limit = limit.unwrap_or(self.prompt_limit);

        let entries: Vec<PersonFactEntry> = facts
            .into_iter()
            .take(limit)
            .map(|f| PersonFactEntry {
                content: f.fact_text,
                category: f.category,
                confidence: f.confidence,
                source: f.source_conversation_id.unwrap_or_default(),
            })
            .collect();

        Ok(entries)
    }

    /// 格式化事实为 prompt 字符串
    pub async fn format_facts_for_prompt(
        &self,
        user_id: &str,
        limit: Option<usize>,
    ) -> XueliResult<String> {
        let entries = self.get_prompt_entries(user_id, limit).await?;
        if entries.is_empty() {
            return Ok(String::new());
        }
        let lines: Vec<String> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| format!("{}. {}", i + 1, e.content))
            .collect();
        Ok(lines.join("\n"))
    }

    /// 从重要记忆元数据构建事实（直接使用 ImportantMemory 的 metadata_json）
    pub fn build_facts_from_important_memories(
        &self,
        user_id: &str,
        important_memories: &[ImportantMemory],
    ) -> Vec<PersonFact> {
        let mut facts = Vec::new();
        let mut seen = HashSet::new();

        for imp in important_memories {
            let content = imp.content.trim();
            if content.len() < 3 {
                continue;
            }
            let normalized = normalize_text(content);
            if normalized.is_empty() {
                continue;
            }

            // 从 metadata_json 中解析分类
            let category = self.infer_category_from_metadata(&imp.metadata_json);

            let key = (category.clone(), normalized.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);

            let id = format!("{}_{}_{}", user_id, category, normalized);
            let now = chrono::Utc::now();

            facts.push(PersonFact {
                id,
                user_id: user_id.to_string(),
                fact_text: content.to_string(),
                category,
                confidence: imp.score,
                source_conversation_id: Some(imp.id.clone()),
                created_at: imp.created_at,
                updated_at: now,
            });
        }

        // 按更新时间降序、创建时间降序、事实文本降序（匹配 Python 语义）
        facts.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.created_at.cmp(&a.created_at))
                .then_with(|| b.fact_text.cmp(&a.fact_text))
        });

        facts
    }

    /// 从 metadata_json 推断分类
    fn infer_category_from_metadata(&self, metadata_json: &str) -> String {
        if metadata_json.is_empty() || metadata_json == "{}" {
            return "profile".to_string();
        }
        let meta: serde_json::Value = match serde_json::from_str(metadata_json) {
            Ok(v) => v,
            Err(_) => return "profile".to_string(),
        };
        // 优先使用 metadata 中的 category 字段
        if let Some(cat) = meta.get("category").and_then(|v| v.as_str()) {
            return cat.to_string();
        }
        if let Some(cat) = meta.get("memory_category").and_then(|v| v.as_str()) {
            return cat.to_string();
        }
        // 根据 insight_type 推断
        if let Some(it) = meta.get("insight_type").and_then(|v| v.as_str()) {
            return match it {
                "digested" => "profile".to_string(),
                "reflection" => "boundary".to_string(),
                _ => "profile".to_string(),
            };
        }
        "profile".to_string()
    }

    /// 从重要记忆构建人物事实（兼容旧接口）
    fn build_facts_from_memories(
        &self,
        user_id: &str,
        memories: &[(ImportantMemory, MemoryItem)],
    ) -> Vec<PersonFact> {
        let mut facts = Vec::new();
        let mut seen = HashSet::new();

        for (important, memory) in memories {
            if !self.should_use_as_fact(memory) {
                continue;
            }

            let category = self.infer_fact_category(memory);
            let content = memory.content.trim();
            let normalized = normalize_text(content);

            if normalized.is_empty() {
                continue;
            }

            let key = (category.clone(), normalized.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);

            let id = format!("{}_{}_{}", user_id, category, normalized);
            let now = chrono::Utc::now();

            facts.push(PersonFact {
                id,
                user_id: user_id.to_string(),
                fact_text: content.to_string(),
                category,
                confidence: important.score,
                source_conversation_id: Some(memory.id.clone()),
                created_at: memory.created_at,
                updated_at: now,
            });
        }

        // 按更新时间降序、创建时间降序、事实文本降序（匹配 Python 语义）
        facts.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.created_at.cmp(&a.created_at))
                .then_with(|| b.fact_text.cmp(&a.fact_text))
        });

        facts
    }

    /// 判断该记忆是否应作为事实
    fn should_use_as_fact(&self, memory: &MemoryItem) -> bool {
        let content = memory.content.trim();
        if content.len() < 3 {
            return false;
        }

        // 排除特定分类（使用 memory_type 判断）
        use crate::core::types::MemoryType;
        match memory.memory_type {
            MemoryType::Fact
            | MemoryType::Preference
            | MemoryType::Opinion
            | MemoryType::Relationship => true,
            MemoryType::Event => false,
        }
    }

    /// 推断事实分类
    fn infer_fact_category(&self, memory: &MemoryItem) -> String {
        use crate::core::types::MemoryType;
        match memory.memory_type {
            MemoryType::Preference => "preference".to_string(),
            MemoryType::Fact => "profile".to_string(),
            MemoryType::Opinion => "boundary".to_string(),
            MemoryType::Relationship => "background".to_string(),
            MemoryType::Event => "plan".to_string(),
        }
    }
}

/// 人物事实条目（用于 prompt）
#[derive(Debug, Clone)]
pub struct PersonFactEntry {
    pub content: String,
    pub category: String,
    pub confidence: f64,
    pub source: String,
}

/// 比较两组事实是否相等（匹配 Python to_dict() 的全字段比较语义）
fn facts_equal(left: &[PersonFact], right: &[PersonFact]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter().zip(right.iter()).all(|(a, b)| {
        a.id == b.id
            && a.user_id == b.user_id
            && a.fact_text == b.fact_text
            && a.category == b.category
            && (a.confidence - b.confidence).abs() < f64::EPSILON
            && a.source_conversation_id == b.source_conversation_id
            && a.created_at == b.created_at
            && a.updated_at == b.updated_at
    })
}

/// 文本归一化（用于去重）
fn normalize_text(text: &str) -> String {
    text.to_lowercase()
        .replace(|c: char| !c.is_alphanumeric(), "")
}
