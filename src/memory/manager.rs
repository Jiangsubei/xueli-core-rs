use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::core::config::MemoryConfig;
use crate::core::types::{MemoryItem, MemoryPatch};
use crate::memory::chat_summary_service::ChatSummaryService;
use crate::memory::internal::access_policy::{MemoryAccessPolicy, PromptEntry};
use crate::memory::internal::index_coordinator::IndexCoordinator;
use crate::memory::internal::task_manager::MemoryTaskManager;
use crate::memory::memory_dispute_resolver::MemoryDisputeResolver;
use crate::memory::person_fact_service::PersonFactService;
use crate::memory::recall_service::ConversationRecallService;
use crate::memory::retrieval::coordinator::{PromptBudgets, RetrievalCoordinator};
use crate::memory::stores::conversation::SqliteConversationStore;
use crate::memory::stores::fact_evidence::SqliteFactEvidenceStore;
use crate::memory::stores::important::{ImportantMemory, ImportantMemoryStore};
use crate::memory::stores::memory_item::SqliteMemoryItemStore;
use crate::memory::stores::person_fact::{PersonFact, SqlitePersonFactStore};
use crate::memory::stores::signal_store::SignalStore;
use crate::memory::stores::traits::MemoryStore;
use crate::prelude::XueliResult;

pub struct MemoryManager {
    config: Arc<MemoryConfig>,

    memory_store: Arc<SqliteMemoryItemStore>,
    important_store: Arc<ImportantMemoryStore>,
    conversation_store: Arc<SqliteConversationStore>,
    person_fact_store: Arc<SqlitePersonFactStore>,
    fact_evidence_store: Arc<SqliteFactEvidenceStore>,
    signal_store: Arc<SignalStore>,

    index_coordinator: Arc<IndexCoordinator>,
    retrieval_coordinator: Arc<RetrievalCoordinator>,

    task_manager: Arc<MemoryTaskManager>,

    chat_summary_service: Arc<ChatSummaryService>,
    person_fact_service: Arc<PersonFactService>,
    recall_service: Arc<ConversationRecallService>,
    dispute_resolver: Arc<MemoryDisputeResolver>,
}

impl MemoryManager {
    pub fn new(
        config: Arc<MemoryConfig>,
        memory_store: Arc<SqliteMemoryItemStore>,
    ) -> XueliResult<Self> {
        let base_dir = Path::new(&config.db_path)
            .parent()
            .unwrap_or(Path::new("."));

        let important_store = Arc::new(ImportantMemoryStore::new(base_dir)?);
        let conversation_store = Arc::new(SqliteConversationStore::open(base_dir)?);
        let person_fact_store = Arc::new(SqlitePersonFactStore::new(base_dir)?);
        let fact_evidence_store = Arc::new(SqliteFactEvidenceStore::new(base_dir)?);
        let signal_db = base_dir.join("signals.db");
        let signal_store = Arc::new(SignalStore::new(&signal_db)?);

        let index_coordinator = Arc::new(IndexCoordinator::new());
        let task_manager = Arc::new(MemoryTaskManager::new());

        let store_trait: Arc<dyn MemoryStore> = memory_store.clone();
        let access_policy = MemoryAccessPolicy::new();
        let prompt_budgets = PromptBudgets::default();
        let retrieval_coordinator = Arc::new(
            RetrievalCoordinator::new(store_trait)
                .with_important_store(important_store.clone())
                .with_access_policy(access_policy)
                .with_prompt_budgets(prompt_budgets),
        );

        let chat_summary_service = Arc::new(ChatSummaryService::new());
        let person_fact_service = Arc::new(PersonFactService::new(
            person_fact_store.clone(),
            important_store.clone(),
            memory_store.clone(),
        ));
        let recall_service = Arc::new(ConversationRecallService::new(conversation_store.clone()));
        let dispute_resolver = Arc::new(MemoryDisputeResolver::new(config.dispute.clone()));

        Ok(Self {
            config,
            memory_store,
            important_store,
            conversation_store,
            person_fact_store,
            fact_evidence_store,
            signal_store,
            index_coordinator,
            retrieval_coordinator,
            task_manager,
            chat_summary_service,
            person_fact_service,
            recall_service,
            dispute_resolver,
        })
    }

    pub async fn initialize(&self) -> XueliResult<()> {
        tracing::debug!("[MemoryManager] 初始化完成");
        Ok(())
    }

    // ── Memory CRUD ──

    pub async fn store(&self, item: MemoryItem) -> XueliResult<String> {
        self.memory_store.store(item).await
    }

    pub async fn store_batch(&self, items: Vec<MemoryItem>) -> XueliResult<Vec<String>> {
        self.memory_store.store_batch(items).await
    }

    pub async fn apply_patch(&self, patch: MemoryPatch) -> XueliResult<()> {
        if !patch.add.is_empty() {
            self.memory_store.store_batch(patch.add).await?;
        }
        for item in patch.update {
            self.memory_store.update(item).await?;
        }
        for id in patch.remove {
            self.memory_store.delete(&id).await?;
        }
        Ok(())
    }

    pub async fn get_user_memories(&self, user_id: &str) -> XueliResult<Vec<MemoryItem>> {
        self.memory_store.get_by_user(user_id).await
    }

    pub async fn get_by_user(&self, user_id: &str) -> XueliResult<Vec<MemoryItem>> {
        self.memory_store.get_by_user(user_id).await
    }

    pub async fn get(&self, id: &str) -> XueliResult<Option<MemoryItem>> {
        self.memory_store.get(id).await
    }

    pub async fn delete(&self, memory_id: &str) -> XueliResult<()> {
        self.memory_store.delete(memory_id).await
    }

    pub async fn search(&self, query: &str, limit: usize) -> XueliResult<Vec<MemoryItem>> {
        self.memory_store.search(query, limit).await
    }

    pub async fn add_memory(
        &self,
        content: &str,
        user_id: &str,
        memory_type: crate::core::types::MemoryType,
    ) -> XueliResult<String> {
        let item = MemoryItem {
            id: uuid_v4(),
            user_id: user_id.to_string(),
            content: content.to_string(),
            memory_type,
            importance: 0.5,
            created_at: chrono::Utc::now(),
            last_accessed_at: chrono::Utc::now(),
            access_count: 0,
        };
        self.memory_store.store(item).await
    }

    pub async fn delete_memory(&self, mem_id: &str, user_id: &str) -> XueliResult<()> {
        if let Some(item) = self.memory_store.get(mem_id).await? {
            if item.user_id == user_id {
                self.memory_store.delete(mem_id).await
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    pub async fn update_memory(
        &self,
        mem_id: &str,
        content: &str,
        user_id: &str,
    ) -> XueliResult<()> {
        if let Some(mut item) = self.memory_store.get(mem_id).await? {
            if item.user_id == user_id {
                item.content = content.to_string();
                self.memory_store.update(item).await
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    // ── Search & Retrieval ──

    pub async fn search_memories(
        &self,
        user_id: &str,
        query: &str,
        top_k: usize,
    ) -> XueliResult<Vec<MemoryItem>> {
        let result = self
            .retrieval_coordinator
            .retrieve(query, user_id, top_k, top_k)
            .await?;
        Ok(result.items)
    }

    pub async fn search_memories_with_context(
        &self,
        user_id: &str,
        query: &str,
        top_k: usize,
    ) -> XueliResult<Vec<PromptEntry>> {
        let context = crate::memory::retrieval::coordinator::MemoryAccessContext::new(
            user_id,
            &crate::core::scope::ChatScope::Private,
        );
        self.retrieval_coordinator
            .search_important_memories(user_id, query, &context, top_k)
            .await
    }

    pub async fn build_prompt_context(
        &self,
        user_id: &str,
        query: &str,
    ) -> XueliResult<crate::memory::retrieval::coordinator::PromptContextResult> {
        let context = crate::memory::retrieval::coordinator::MemoryAccessContext::new(
            user_id,
            &crate::core::scope::ChatScope::Private,
        );
        let section_policy: HashMap<String, bool> = [
            ("session_restore".to_string(), false),
            ("precise_recall".to_string(), false),
            ("dynamic".to_string(), true),
        ]
        .into_iter()
        .collect();
        let section_intensity: HashMap<String, String> = [
            ("session_restore".to_string(), "normal".to_string()),
            ("precise_recall".to_string(), "normal".to_string()),
            ("dynamic".to_string(), "normal".to_string()),
        ]
        .into_iter()
        .collect();
        self.retrieval_coordinator
            .build_prompt_context(
                user_id,
                query,
                &context,
                &section_policy,
                &section_intensity,
                "",
            )
            .await
    }

    pub async fn quick_check_relevance(
        &self,
        user_id: &str,
        query: &str,
        threshold: f64,
    ) -> XueliResult<Option<MemoryItem>> {
        let items = self.memory_store.get_by_user(user_id).await?;
        for item in &items {
            let score = text_match_score(query, &item.content);
            if score >= threshold {
                return Ok(Some(item.clone()));
            }
        }
        Ok(None)
    }

    // ── Important Memories ──

    pub async fn add_important_memory(
        &self,
        user_id: &str,
        content: &str,
        source: &str,
        priority: i32,
    ) -> XueliResult<Option<ImportantMemory>> {
        self.important_store
            .add_memory(user_id, content, source, priority, None)
            .await
    }

    pub async fn get_important_memories(
        &self,
        user_id: &str,
        min_priority: i32,
        limit: usize,
    ) -> XueliResult<Vec<ImportantMemory>> {
        self.important_store
            .get_memories(user_id, min_priority)
            .await
            .map(|mut v| {
                v.truncate(limit);
                v
            })
    }

    pub async fn search_important_memories(
        &self,
        user_id: &str,
        query: &str,
    ) -> XueliResult<Vec<ImportantMemory>> {
        self.important_store.search_memories(user_id, query).await
    }

    pub async fn delete_important_memory(&self, mem_id: &str) -> XueliResult<bool> {
        self.important_store.delete_memory(mem_id).await
    }

    pub async fn update_important_memory(&self, mem_id: &str, content: &str) -> XueliResult<bool> {
        self.important_store.update_memory(mem_id, content).await
    }

    pub async fn clear_important_memories(&self, user_id: &str) -> XueliResult<usize> {
        self.important_store.clear_memories(user_id).await
    }

    pub async fn format_important_memories_for_prompt(&self, user_id: &str) -> XueliResult<String> {
        let context = crate::memory::retrieval::coordinator::MemoryAccessContext::new(
            user_id,
            &crate::core::scope::ChatScope::Private,
        );
        self.retrieval_coordinator
            .format_important_memories_for_prompt(user_id, &context, 5)
            .await
    }

    // ── Recall Tracking ──

    pub async fn mark_recalled_memories(
        &self,
        user_id: &str,
        memory_ids: &[String],
    ) -> XueliResult<usize> {
        let mut total = 0usize;
        for mid in memory_ids {
            if mid.starts_with("imp_") {
                self.important_store.mark_recalled(mid).await?;
                total += 1;
            } else {
                tracing::debug!(
                    "[MemoryManager] 普通记忆标记召回（暂未实现）: user={} id={}",
                    user_id,
                    mid
                );
            }
        }
        if total > 0 {
            tracing::debug!("[MemoryManager] 记忆召回回写完成: {} 条", total);
        }
        Ok(total)
    }

    // ── Person Facts ──

    pub async fn get_person_facts(&self, user_id: &str) -> XueliResult<Vec<PersonFact>> {
        self.person_fact_store.get_by_user(user_id).await
    }

    pub async fn format_person_facts_for_prompt(&self, user_id: &str) -> XueliResult<String> {
        let facts = self.person_fact_store.get_by_user(user_id).await?;
        if facts.is_empty() {
            return Ok(String::new());
        }
        let mut lines = vec!["=== 关于用户的已知信息 ===".to_string()];
        for (i, fact) in facts.iter().take(6).enumerate() {
            lines.push(format!("{}. [{}] {}", i + 1, fact.category, fact.fact_text));
        }
        Ok(lines.join("\n"))
    }

    pub async fn get_person_fact_prompt_entries(
        &self,
        user_id: &str,
        limit: usize,
    ) -> XueliResult<Vec<HashMap<String, serde_json::Value>>> {
        let facts = self.person_fact_store.get_by_user(user_id).await?;
        let entries: Vec<_> = facts
            .into_iter()
            .take(limit)
            .map(|f| {
                let mut m = HashMap::new();
                m.insert(
                    "content".to_string(),
                    serde_json::Value::String(f.fact_text),
                );
                m.insert(
                    "category".to_string(),
                    serde_json::Value::String(f.category),
                );
                m.insert(
                    "confidence".to_string(),
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(f.confidence).unwrap_or(0.into()),
                    ),
                );
                m
            })
            .collect();
        Ok(entries)
    }

    // ── Background Tasks (stubs — awaiting BackgroundCoordinator fix) ──

    pub async fn register_dialogue_turn(
        &self,
        user_id: &str,
        user_message: &str,
        assistant_message: &str,
        _dialogue_key: &str,
        _scope_type: &str,
        _group_id: &str,
        _message_id: &str,
        _image_description: &str,
        _narrative_summary: &str,
        _platform: &str,
    ) -> XueliResult<()> {
        tracing::debug!(
            "[MemoryManager] 注册对话轮次 user={} (完整管线待接入)",
            user_id
        );
        let _ = (user_id, user_message, assistant_message);
        Ok(())
    }

    pub async fn maybe_extract_memories(&self, user_id: &str) -> XueliResult<()> {
        tracing::debug!(
            "[MemoryManager] 可能触发记忆提取 user={} (待接入 extractor)",
            user_id
        );
        Ok(())
    }

    pub async fn force_extraction(&self, user_id: &str) -> XueliResult<()> {
        tracing::debug!(
            "[MemoryManager] 强制记忆提取 user={} (待接入 extractor)",
            user_id
        );
        Ok(())
    }

    pub async fn flush_conversation_session(&self, user_id: &str) -> XueliResult<()> {
        tracing::debug!(
            "[MemoryManager] 刷新对话会话 user={} (待接入完整管线)",
            user_id
        );
        Ok(())
    }

    // ── Index Management ──

    pub async fn rebuild_index(&self, user_id: &str) -> XueliResult<()> {
        let memories = self.memory_store.get_by_user(user_id).await?;
        let docs: Vec<(String, String)> = memories
            .iter()
            .map(|m| (m.id.clone(), m.content.clone()))
            .collect();
        self.index_coordinator.rebuild(&docs).await
    }

    pub async fn rebuild_all_indices(&self) -> XueliResult<()> {
        let user_ids = self.get_all_user_ids().await?;
        let mut all_docs: Vec<(String, String)> = Vec::new();
        for uid in &user_ids {
            if let Ok(memories) = self.memory_store.get_by_user(uid).await {
                for m in &memories {
                    all_docs.push((m.id.clone(), m.content.clone()));
                }
            }
        }
        self.index_coordinator.rebuild(&all_docs).await
    }

    pub async fn mark_index_dirty(&self, user_id: &str) {
        self.index_coordinator.mark_dirty(user_id).await
    }

    pub async fn ensure_index_fresh(&self, user_id: &str) -> XueliResult<()> {
        let memories = self.memory_store.get_by_user(user_id).await?;
        let docs: Vec<(String, String)> = memories
            .iter()
            .map(|m| (m.id.clone(), m.content.clone()))
            .collect();
        self.index_coordinator.ensure_fresh(&docs).await
    }

    async fn get_all_user_ids(&self) -> XueliResult<Vec<String>> {
        let items = self.memory_store.get_by_user("").await?;
        let user_ids: std::collections::HashSet<String> = items
            .into_iter()
            .map(|m| m.user_id)
            .filter(|u| !u.is_empty())
            .collect();
        Ok(user_ids.into_iter().collect())
    }

    // ── Access & Utility ──

    pub fn important_store(&self) -> Arc<ImportantMemoryStore> {
        self.important_store.clone()
    }

    pub fn conversation_store(&self) -> Arc<SqliteConversationStore> {
        self.conversation_store.clone()
    }

    pub fn person_fact_store(&self) -> Arc<SqlitePersonFactStore> {
        self.person_fact_store.clone()
    }

    pub fn index_coordinator(&self) -> Arc<IndexCoordinator> {
        self.index_coordinator.clone()
    }

    pub fn retrieval_coordinator(&self) -> Arc<RetrievalCoordinator> {
        self.retrieval_coordinator.clone()
    }

    pub async fn close(&self) {
        tracing::debug!("[MemoryManager] 已关闭");
    }

    pub async fn get_stats(&self) -> HashMap<String, String> {
        let mut stats = HashMap::new();
        stats.insert("db_path".to_string(), self.config.db_path.clone());
        stats.insert(
            "background_tasks".to_string(),
            self.task_manager.count().await.to_string(),
        );
        stats
    }
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn text_match_score(query: &str, content: &str) -> f64 {
    let nq: String = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    let nc: String = content
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();

    if nq.is_empty() || nc.is_empty() {
        return 0.0;
    }
    if nc.contains(&nq) || nq.contains(&nc) {
        return nq.len().min(nc.len()) as f64 / nq.len().max(nc.len()) as f64;
    }
    let q_chars: std::collections::HashSet<char> = nq.chars().collect();
    let c_chars: std::collections::HashSet<char> = nc.chars().collect();
    let overlap = q_chars.intersection(&c_chars).count();
    overlap as f64 / c_chars.len().max(1) as f64
}
