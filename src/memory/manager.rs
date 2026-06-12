use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::core::config::MemoryConfig;
use crate::core::scope::ChatScope;
use crate::core::types::{MemoryItem, MemoryPatch, MemoryType};
use crate::memory::chat_summary_service::ChatSummaryService;
use crate::memory::internal::access_policy::{
    MemoryAccessContext, MemoryAccessPolicy, PromptEntry,
};
use crate::memory::internal::background::MemoryBackgroundCoordinator;
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
use crate::traits::prompt_template::PromptTemplateLoader;

pub struct MemoryManager<L: PromptTemplateLoader + 'static> {
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

    background_coordinator: Option<Arc<MemoryBackgroundCoordinator<L>>>,
}

impl<L: PromptTemplateLoader + 'static> MemoryManager<L> {
    pub fn new(
        config: Arc<MemoryConfig>,
        memory_store: Arc<SqliteMemoryItemStore>,
        prompt_loader: Arc<L>,
    ) -> XueliResult<Self> {
        let base_dir = Path::new(&config.data_dir);

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

        // 构建后台协调器
        let background_coordinator =
            MemoryBackgroundCoordinator::new(config.clone(), task_manager.clone(), prompt_loader)
                .with_conversation_store(conversation_store.clone())
                .with_memory_store(memory_store.clone())
                .with_important_store(important_store.clone())
                .with_person_fact_service(person_fact_service.clone())
                .with_summary_service(chat_summary_service.clone())
                .with_auto_extract(config.auto_extract)
                .into_arc();

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
            background_coordinator: Some(background_coordinator),
        })
    }

    /// 设置 LLM 客户端（启用记忆提取和消化功能）
    pub fn with_llm_client(
        self: Arc<Self>,
        _client: Arc<dyn crate::traits::ai_client::AIClient>,
        _model: String,
    ) -> Arc<Self> {
        // 由于 background_coordinator 已在 new() 中创建，
        // 需要通过 Arc 内部可变性来更新 coordinator 的 LLM 客户端
        // 当前实现中 coordinator 的 llm_client 是 Option，通过 builder 设置
        // 这里我们重新构建 coordinator
        if let Some(ref _coord) = self.background_coordinator {
            // coordinator 的 llm_client 在构建时已设为 None，
            // 需要在构建时传入。此处通过 rebuild 方式处理。
            tracing::info!("[MemoryManager] LLM 客户端设置需在构建时完成");
        }
        self
    }

    pub async fn initialize(&self) -> XueliResult<()> {
        tracing::debug!("[MemoryManager] 开始初始化");

        // 同步已有用户的记忆到人物事实
        self.sync_existing_person_facts().await;

        // 构建索引
        self.rebuild_all_indices().await?;

        // 启动后台消化循环
        if let Some(ref coord) = self.background_coordinator {
            coord.start(300); // 默认 300 秒消化间隔
        }

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
        memory_type: MemoryType,
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
        let id = self.memory_store.store(item).await?;
        if !user_id.is_empty() {
            self.mark_index_dirty(user_id).await;
        }
        Ok(id)
    }

    pub async fn delete_memory(&self, mem_id: &str, user_id: &str) -> XueliResult<()> {
        if let Some(item) = self.memory_store.get(mem_id).await? {
            if item.user_id == user_id {
                self.memory_store.delete(mem_id).await?;
                if !user_id.is_empty() {
                    self.mark_index_dirty(user_id).await;
                }
            }
        }
        Ok(())
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
                self.memory_store.update(item).await?;
                if !user_id.is_empty() {
                    self.mark_index_dirty(user_id).await;
                }
            }
        }
        Ok(())
    }

    // ── Search & Retrieval ──

    pub async fn search_memories(
        &self,
        user_id: &str,
        query: &str,
        top_k: usize,
        scope: &ChatScope,
    ) -> XueliResult<Vec<MemoryItem>> {
        self.retrieval_coordinator
            .search_memories(user_id, query, top_k, scope)
            .await
    }

    pub async fn search_memories_with_context(
        &self,
        user_id: &str,
        query: &str,
        top_k: usize,
    ) -> XueliResult<Vec<PromptEntry>> {
        let context = MemoryAccessContext::new(user_id, &ChatScope::Private);
        self.retrieval_coordinator
            .search_important_memories(user_id, query, &context, top_k)
            .await
    }

    pub async fn build_prompt_context(
        &self,
        user_id: &str,
        query: &str,
    ) -> XueliResult<crate::memory::retrieval::coordinator::PromptContextResult> {
        let context = MemoryAccessContext::new(user_id, &ChatScope::Private);
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
        self.retrieval_coordinator
            .quick_check_relevance(user_id, query, threshold)
            .await
    }

    // ── Important Memories ──

    pub async fn add_important_memory(
        &self,
        user_id: &str,
        content: &str,
        source: &str,
        priority: i32,
    ) -> XueliResult<Option<ImportantMemory>> {
        let result = self
            .important_store
            .add_memory(user_id, content, source, priority, None)
            .await?;
        if result.is_some() {
            if let Err(e) = self.person_fact_service.sync_user_facts(user_id).await {
                tracing::warn!("[MemoryManager] 同步人物事实失败: {}", e);
            }
        }
        Ok(result)
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
        let result = self.important_store.delete_memory(mem_id).await?;
        if result {
            // 无法从 mem_id 反推 user_id，跳过人物事实同步
        }
        Ok(result)
    }

    pub async fn delete_important_memory_by_id(
        &self,
        user_id: &str,
        mem_id: &str,
    ) -> XueliResult<bool> {
        let result = self
            .important_store
            .delete_memory_by_id(user_id, mem_id)
            .await?;
        if result {
            if let Err(e) = self.person_fact_service.sync_user_facts(user_id).await {
                tracing::warn!("[MemoryManager] 同步人物事实失败: {}", e);
            }
        }
        Ok(result)
    }

    pub async fn update_important_memory(&self, mem_id: &str, content: &str) -> XueliResult<bool> {
        let result = self.important_store.update_memory(mem_id, content).await?;
        // 无法从 mem_id 反推 user_id，跳过人物事实同步
        Ok(result)
    }

    pub async fn clear_important_memories(&self, user_id: &str) -> XueliResult<usize> {
        let count = self.important_store.clear_memories(user_id).await?;
        if count > 0 {
            // 清空该用户的人物事实
            let facts = self.person_fact_store.get_by_user(user_id).await?;
            for fact in &facts {
                if let Err(e) = self.person_fact_store.delete(&fact.id).await {
                    tracing::warn!("[MemoryManager] 删除人物事实失败: {}", e);
                }
            }
        }
        Ok(count)
    }

    pub async fn format_important_memories_for_prompt(&self, user_id: &str) -> XueliResult<String> {
        let context = MemoryAccessContext::new(user_id, &ChatScope::Private);
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
        if memory_ids.is_empty() {
            return Ok(0);
        }

        let ordinary_ids: Vec<&String> = memory_ids
            .iter()
            .filter(|mid| !mid.starts_with("imp_"))
            .collect();
        let important_ids: Vec<&String> = memory_ids
            .iter()
            .filter(|mid| mid.starts_with("imp_"))
            .collect();

        let mut total = 0usize;

        // 普通记忆标记召回
        if !ordinary_ids.is_empty() {
            let ids: Vec<String> = ordinary_ids.into_iter().cloned().collect();
            match self.memory_store.mark_recalled(user_id, &ids).await {
                Ok(count) => {
                    total += count;
                    self.mark_index_dirty(user_id).await;
                }
                Err(e) => {
                    tracing::warn!("[MemoryManager] 普通记忆标记召回失败: {}", e);
                }
            }
        }

        // 重要记忆标记召回
        if !important_ids.is_empty() {
            let ids: Vec<String> = important_ids.into_iter().cloned().collect();
            match self
                .important_store
                .mark_recalled_batch(user_id, &ids)
                .await
            {
                Ok(count) => total += count,
                Err(e) => {
                    tracing::warn!("[MemoryManager] 重要记忆标记召回失败: {}", e);
                }
            }
        }

        if total > 0 {
            tracing::debug!("[MemoryManager] 记忆召回回写完成: {} 条", total);
        }
        Ok(total)
    }

    /// 调度后台任务，异步标记记忆被召回
    pub fn schedule_mark_recalled(&self, user_id: String, memory_ids: Vec<String>) {
        if memory_ids.is_empty() {
            return;
        }
        let memory_store = self.memory_store.clone();
        let important_store = self.important_store.clone();
        let index_coordinator = self.index_coordinator.clone();
        let uid = user_id.clone();
        let uid_for_label = uid.clone();
        self.task_manager.create_task(
            async move {
                let ordinary_ids: Vec<String> = memory_ids
                    .iter()
                    .filter(|mid| !mid.starts_with("imp_"))
                    .cloned()
                    .collect();
                let important_ids: Vec<String> = memory_ids
                    .iter()
                    .filter(|mid| mid.starts_with("imp_"))
                    .cloned()
                    .collect();

                if !ordinary_ids.is_empty() {
                    if let Ok(count) = memory_store.mark_recalled(&uid, &ordinary_ids).await {
                        if count > 0 {
                            index_coordinator.mark_dirty(&uid).await;
                        }
                    }
                }
                if !important_ids.is_empty() {
                    let _ = important_store
                        .mark_recalled_batch(&uid, &important_ids)
                        .await;
                }
            },
            Some(format!("memory-mark-recalled-{}", uid_for_label)),
        );
    }

    // ── Suppress Competitors ──

    pub async fn suppress_competitors(
        &self,
        user_id: &str,
        selected_ids: &[String],
        penalty: f64,
    ) -> XueliResult<usize> {
        if !self.config.suppression.enabled {
            return Ok(0);
        }
        self.memory_store
            .suppress_competitors(user_id, selected_ids, penalty)
            .await
    }

    // ── Person Facts ──

    pub async fn get_person_facts(&self, user_id: &str) -> XueliResult<Vec<PersonFact>> {
        self.person_fact_store.get_by_user(user_id).await
    }

    pub async fn format_person_facts_for_prompt(&self, user_id: &str) -> XueliResult<String> {
        self.person_fact_service
            .format_facts_for_prompt(user_id, None)
            .await
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

    // ── Background Tasks ──

    #[allow(clippy::too_many_arguments)]
    pub async fn register_dialogue_turn(
        &self,
        user_id: &str,
        user_message: &str,
        assistant_message: &str,
        dialogue_key: &str,
        scope_type: &str,
        group_id: &str,
        message_id: &str,
        _image_description: &str,
        narrative_summary: &str,
        platform: &str,
    ) -> XueliResult<()> {
        if let Some(ref coord) = self.background_coordinator {
            // 解析 session_id
            let session_id = coord.resolve_session_id(
                user_id,
                Some(dialogue_key),
                scope_type,
                if group_id.is_empty() {
                    None
                } else {
                    Some(group_id)
                },
                None,
                platform,
            );
            let turn_id = 0; // 由 conversation_store 自动分配

            coord
                .register_dialogue_turn(
                    user_id,
                    user_message,
                    assistant_message,
                    &session_id,
                    turn_id,
                    dialogue_key,
                    scope_type,
                    group_id,
                    message_id,
                    narrative_summary,
                    platform,
                )
                .await;
        } else {
            tracing::debug!(
                "[MemoryManager] 注册对话轮次 user={} (后台协调器未启用)",
                user_id
            );
        }
        Ok(())
    }

    pub async fn maybe_extract_memories(&self, user_id: &str, session_id: &str) -> Vec<MemoryItem> {
        if let Some(ref coord) = self.background_coordinator {
            coord.maybe_extract_memories(user_id, session_id).await
        } else {
            tracing::debug!(
                "[MemoryManager] 可能触发记忆提取 user={} (后台协调器未启用)",
                user_id
            );
            vec![]
        }
    }

    pub fn schedule_memory_extraction(&self, user_id: &str, session_id: &str) {
        if let Some(ref coord) = self.background_coordinator {
            coord.schedule_memory_extraction(user_id.to_string(), session_id.to_string());
        }
    }

    pub fn force_extraction(&self, user_id: &str, session_id: &str) {
        if let Some(ref coord) = self.background_coordinator {
            coord.force_extraction(user_id.to_string(), session_id.to_string());
        } else {
            tracing::debug!(
                "[MemoryManager] 强制记忆提取 user={} (后台协调器未启用)",
                user_id
            );
        }
    }

    pub fn flush_conversation_session(&self, user_id: &str, session_id: &str) {
        if let Some(ref coord) = self.background_coordinator {
            coord.flush_conversation_session(user_id.to_string(), session_id.to_string());
        } else {
            tracing::debug!(
                "[MemoryManager] 刷新对话会话 user={} (后台协调器未启用)",
                user_id
            );
        }
    }

    pub async fn flush_background_tasks(&self) {
        if let Some(ref coord) = self.background_coordinator {
            coord.flush().await;
        }
    }

    // ── Access Context ──

    pub fn build_access_context(
        &self,
        user_id: &str,
        message_type: &str,
        group_id: Option<&str>,
        read_scope: Option<&str>,
        platform: &str,
        hour_of_day: i32,
    ) -> MemoryAccessContext {
        MemoryAccessPolicy::build_context(
            user_id,
            message_type,
            group_id.unwrap_or(""),
            read_scope.unwrap_or("user"),
            platform,
            hour_of_day,
        )
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
        self.memory_store.get_all_user_ids().await
    }

    // ── Internal: Person Fact Sync ──

    async fn sync_existing_person_facts(&self) {
        let memory_user_ids = match self.memory_store.get_all_user_ids().await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!("[MemoryManager] 获取记忆用户列表失败: {}", e);
                return;
            }
        };
        let important_user_ids = match self.important_store.get_user_ids().await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!("[MemoryManager] 获取重要记忆用户列表失败: {}", e);
                return;
            }
        };

        let all_user_ids: std::collections::HashSet<String> = memory_user_ids
            .into_iter()
            .chain(important_user_ids.into_iter())
            .filter(|u| !u.is_empty())
            .collect();

        for user_id in &all_user_ids {
            if let Err(e) = self.person_fact_service.sync_user_facts(user_id).await {
                tracing::warn!("[MemoryManager] 同步用户 {} 人物事实失败: {}", user_id, e);
            }
        }
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

    pub fn fact_evidence_store(&self) -> Arc<SqliteFactEvidenceStore> {
        self.fact_evidence_store.clone()
    }

    pub fn signal_store(&self) -> Arc<SignalStore> {
        self.signal_store.clone()
    }

    pub fn chat_summary_service(&self) -> Arc<ChatSummaryService> {
        self.chat_summary_service.clone()
    }

    pub fn recall_service(&self) -> Arc<ConversationRecallService> {
        self.recall_service.clone()
    }

    pub fn dispute_resolver(&self) -> Arc<MemoryDisputeResolver> {
        self.dispute_resolver.clone()
    }

    pub fn background_coordinator(&self) -> Option<Arc<MemoryBackgroundCoordinator<L>>> {
        self.background_coordinator.clone()
    }

    // ── Dispute Resolution ──

    /// 解析记忆争议（使用 dispute_resolver + fact_evidence_store）
    pub async fn resolve_memory_dispute(
        &self,
        user_id: &str,
        memory_id: &str,
        memory_metadata: &serde_json::Value,
    ) -> XueliResult<Option<crate::memory::stores::fact_evidence::FactEvidence>> {
        let decision = self
            .dispute_resolver
            .resolve_from_memory_metadata(memory_metadata);
        if decision.level == "ignore" {
            return Ok(None);
        }
        let record = crate::memory::stores::fact_evidence::FactEvidence {
            id: format!("ev_{}_{}", user_id, memory_id),
            fact_id: memory_id.to_string(),
            source_memory_id: memory_id.to_string(),
            conversation_id: String::new(),
            message_id: String::new(),
            evidence_text: decision.summary.clone(),
            created_at: chrono::Utc::now(),
        };
        self.fact_evidence_store.store(record.clone()).await?;
        Ok(Some(record))
    }

    // ── Recall Context ──

    /// 获取对话回溯上下文（使用 recall_service）
    pub async fn get_recall_context(
        &self,
        user_id: &str,
        session_id: &str,
        query: &str,
    ) -> XueliResult<Vec<MemoryItem>> {
        self.recall_service.recall(user_id, session_id, query).await
    }

    // ── Chat Summary ──

    /// 刷新会话摘要（使用 chat_summary_service）
    pub async fn refresh_chat_summary(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> XueliResult<Option<String>> {
        self.chat_summary_service
            .refresh_session_summary(&self.conversation_store, session_id, user_id)
            .await
    }

    // ── Signal Store ──

    /// 设置信号（使用 signal_store）
    pub async fn set_signal(
        &self,
        signal_key: &str,
        signal_type: &str,
        prompt_version: &str,
        payload: &serde_json::Value,
        confidence: f64,
        ttl_seconds: f64,
    ) -> XueliResult<()> {
        self.signal_store
            .set(
                signal_key,
                signal_type,
                prompt_version,
                payload,
                confidence,
                ttl_seconds,
            )
            .await
    }

    /// 获取信号（使用 signal_store）
    pub async fn get_signal(&self, signal_key: &str) -> Option<serde_json::Value> {
        self.signal_store.get(signal_key).await
    }

    /// 获取信号元数据（使用 signal_store）
    pub async fn get_signal_meta(
        &self,
        signal_key: &str,
    ) -> Option<crate::memory::stores::signal_store::SignalMeta> {
        self.signal_store.get_meta(signal_key).await
    }

    pub async fn close(&self) {
        if let Some(ref coord) = self.background_coordinator {
            coord.close().await;
        }
        tracing::debug!("[MemoryManager] 已关闭");
    }

    pub async fn get_stats(&self) -> HashMap<String, String> {
        let mut stats = HashMap::new();
        stats.insert("data_dir".to_string(), self.config.data_dir.clone());
        stats.insert(
            "background_tasks".to_string(),
            self.task_manager.count().await.to_string(),
        );
        stats.insert(
            "auto_extract".to_string(),
            self.config.auto_extract.to_string(),
        );
        stats.insert(
            "background_coordinator_running".to_string(),
            self.background_coordinator
                .as_ref()
                .map(|c| c.is_running().to_string())
                .unwrap_or_else(|| "none".to_string()),
        );
        stats
    }
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}
