use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::scope::ChatScope;
use crate::core::types::MemoryItem;
use crate::memory::extraction::chat_summary::ChatSummaryService;
use crate::memory::internal::access_policy::{MemoryAccessPolicy, PromptEntry};
use crate::memory::recall_service::ConversationRecallService;
use crate::memory::retrieval::bm25_index::BM25Index;
use crate::memory::retrieval::recall_renderer::RecallRenderer;
use crate::memory::retrieval::vector_index::VectorIndex;
use crate::memory::stores::conversation::{ConversationRecord, SqliteConversationStore};
use crate::memory::stores::important::ImportantMemoryStore;
use crate::memory::stores::traits::MemoryStore;
use crate::prelude::XueliResult;

/// 提示词各部分字符预算
#[derive(Debug, Clone)]
pub struct PromptBudgets {
    pub user_important: usize,
    pub addressing: usize,
    pub shared: usize,
    pub session_restore: usize,
    pub precise_recall: usize,
    pub dynamic: usize,
}

impl Default for PromptBudgets {
    fn default() -> Self {
        Self {
            user_important: 360,
            addressing: 180,
            shared: 260,
            session_restore: 260,
            precise_recall: 260,
            dynamic: 420,
        }
    }
}

// 重导出 access_policy 中的 MemoryAccessContext，统一类型
pub use crate::memory::internal::access_policy::MemoryAccessContext;

/// 构建提示词的记忆上下文结果
#[derive(Debug, Clone)]
pub struct PromptContextResult {
    /// 格式化后的提示词文本
    pub prompt_text: String,
    /// 各部分的条目
    pub sections: HashMap<String, Vec<PromptEntry>>,
    /// session restore 条目
    pub session_restore: Vec<PromptEntry>,
    /// precise recall 条目
    pub precise_recall: Vec<PromptEntry>,
    /// dynamic 记忆条目
    pub dynamic_memories: Vec<PromptEntry>,
    /// 被使用的记忆 ID 列表
    pub used_memory_ids: Vec<String>,
}

/// 检索协调器 — 统一编排 BM25 + 向量混合检索与提示词上下文组装
pub struct RetrievalCoordinator {
    store: Arc<dyn MemoryStore>,
    important_store: Option<Arc<ImportantMemoryStore>>,
    access_policy: MemoryAccessPolicy,
    bm25: RwLock<BM25Index>,
    vector: RwLock<VectorIndex>,
    /// 文档 ID → MemoryItem ID 映射
    doc_to_memory: RwLock<HashMap<String, String>>,
    prompt_budgets: PromptBudgets,
    /// 会话存储（用于 session restore）
    conversation_store: Option<Arc<SqliteConversationStore>>,
    /// 对话回忆服务（用于 precise recall）
    recall_service: Option<Arc<ConversationRecallService>>,
    /// 模糊回忆渲染器
    recall_renderer: Option<RecallRenderer>,
    /// 是否启用动态记忆去重
    dynamic_dedup_enabled: bool,
}

/// 检索结果
#[derive(Debug, Clone)]
pub struct RetrievalResult {
    pub items: Vec<MemoryItem>,
    pub scores: Vec<f64>,
}

impl RetrievalCoordinator {
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self {
            store,
            important_store: None,
            access_policy: MemoryAccessPolicy::new(),
            bm25: RwLock::new(BM25Index::new(1.5, 0.75)),
            vector: RwLock::new(VectorIndex::default()),
            doc_to_memory: RwLock::new(HashMap::new()),
            prompt_budgets: PromptBudgets::default(),
            conversation_store: None,
            recall_service: None,
            recall_renderer: None,
            dynamic_dedup_enabled: true,
        }
    }

    /// 设置重要记忆存储
    pub fn with_important_store(mut self, store: Arc<ImportantMemoryStore>) -> Self {
        self.important_store = Some(store);
        self
    }

    /// 设置提示词预算
    pub fn with_prompt_budgets(mut self, budgets: PromptBudgets) -> Self {
        self.prompt_budgets = budgets;
        self
    }

    /// 设置访问策略
    pub fn with_access_policy(mut self, policy: MemoryAccessPolicy) -> Self {
        self.access_policy = policy;
        self
    }

    /// 设置会话存储（用于 session restore）
    pub fn with_conversation_store(mut self, store: Arc<SqliteConversationStore>) -> Self {
        self.conversation_store = Some(store);
        self
    }

    /// 设置回忆服务（用于 precise recall）
    pub fn with_recall_service(mut self, service: Arc<ConversationRecallService>) -> Self {
        self.recall_service = Some(service);
        self
    }

    /// 设置模糊回忆渲染器
    pub fn with_recall_renderer(mut self, renderer: RecallRenderer) -> Self {
        self.recall_renderer = Some(renderer);
        self
    }

    /// 设置动态记忆去重开关
    pub fn with_dynamic_dedup(mut self, enabled: bool) -> Self {
        self.dynamic_dedup_enabled = enabled;
        self
    }

    // ── 基础索引操作 ──────────

    /// 将记忆条目添加到 BM25 + 向量索引
    pub async fn index_item(&self, item: &MemoryItem, doc_id: &str) {
        let mut bm25 = self.bm25.write().await;
        bm25.add(doc_id.to_string(), &item.content);

        let mut vector = self.vector.write().await;
        vector.add(doc_id.to_string(), &item.content);

        let mut map = self.doc_to_memory.write().await;
        map.insert(doc_id.to_string(), item.id.clone());
    }

    /// 批量索引记忆条目
    pub async fn index_items(&self, items: &[MemoryItem]) {
        let mut bm25 = self.bm25.write().await;
        let mut vector = self.vector.write().await;
        let mut map = self.doc_to_memory.write().await;

        for (i, item) in items.iter().enumerate() {
            let doc_id = format!("doc_{}", i);
            bm25.add(doc_id.clone(), &item.content);
            vector.add(doc_id.clone(), &item.content);
            map.insert(doc_id, item.id.clone());
        }
    }

    /// 执行混合检索（BM25 + 向量）
    pub async fn retrieve(
        &self,
        query: &str,
        user_id: &str,
        bm25_top_k: usize,
        vector_top_k: usize,
    ) -> XueliResult<RetrievalResult> {
        let bm25 = self.bm25.read().await;
        let vector = self.vector.read().await;
        let doc_map = self.doc_to_memory.read().await;

        let bm25_results = bm25.search(query, bm25_top_k, 0.0);
        let vector_results = vector.search(query, vector_top_k);

        // 合并 BM25 + 向量分数（取两者中较高值）
        let mut merged_scores: HashMap<String, f64> = HashMap::new();
        for (doc_id, score) in &bm25_results {
            merged_scores
                .entry(doc_id.clone())
                .and_modify(|s| *s = s.max(*score))
                .or_insert(*score);
        }
        for (doc_id, score) in &vector_results {
            merged_scores
                .entry(doc_id.clone())
                .and_modify(|s| *s = s.max(*score))
                .or_insert(*score);
        }

        let mut scored_docs: Vec<(String, f64)> = merged_scores.into_iter().collect();
        scored_docs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut items = Vec::new();
        let mut scores = Vec::new();

        for (doc_id, score) in scored_docs {
            if let Some(memory_id) = doc_map.get(&doc_id) {
                if let Some(item) = self.store.get(memory_id).await? {
                    if item.user_id == user_id || item.user_id.is_empty() {
                        items.push(item);
                        scores.push(score);
                    }
                }
            }
        }

        Ok(RetrievalResult { items, scores })
    }

    /// 清空索引（用于重建）
    pub async fn clear_index(&self) {
        let mut bm25 = self.bm25.write().await;
        *bm25 = BM25Index::new(1.5, 0.75);

        let mut vector = self.vector.write().await;
        *vector = VectorIndex::default();

        let mut map = self.doc_to_memory.write().await;
        map.clear();
    }

    /// 搜索记忆：跨作用域检索，过滤不可访问结果，按分数排序返回
    pub async fn search_memories(
        &self,
        user_id: &str,
        query: &str,
        top_k: usize,
        scope: &ChatScope,
    ) -> XueliResult<Vec<MemoryItem>> {
        let _context = MemoryAccessContext::new(user_id, scope);

        let retrieval_result = self.retrieve(query, user_id, top_k * 2, top_k * 2).await?;

        let mut filtered: Vec<MemoryItem> = Vec::new();
        for item in retrieval_result.items {
            if self.access_policy.is_accessible(&item, scope) {
                filtered.push(item);
            }
            if filtered.len() >= top_k {
                break;
            }
        }

        Ok(filtered)
    }

    /// 快速检查特定查询是否与某条记忆显著相关，返回首个匹配的记忆
    pub async fn quick_check_relevance(
        &self,
        user_id: &str,
        query: &str,
        threshold: f64,
    ) -> XueliResult<Option<MemoryItem>> {
        let result = self.retrieve(query, user_id, 5, 5).await?;

        for (i, item) in result.items.iter().enumerate() {
            let score = result.scores.get(i).copied().unwrap_or(0.0);
            if score >= threshold {
                return Ok(Some(item.clone()));
            }
        }

        // 回退：对普通记忆做文本相关性评分
        let all_memories = self.store.get_by_user(user_id).await?;
        let mut best: Option<(f64, MemoryItem)> = None;
        for mem in all_memories {
            let score = self.score_text(query, &mem.content);
            if score >= threshold {
                match &best {
                    Some((best_score, _)) if score <= *best_score => {}
                    _ => best = Some((score, mem)),
                }
            }
        }

        Ok(best.map(|(_, item)| item))
    }

    // ── 提示词上下文组装 ──────────

    /// 搜索记忆并组装完整的提示词上下文
    pub async fn search_memories_with_context(
        &self,
        user_id: &str,
        query: &str,
        scope: &ChatScope,
        include_conversations: bool,
        user_emotion_label: &str,
    ) -> XueliResult<PromptContextResult> {
        let context = MemoryAccessContext::new(user_id, scope);
        let section_policy: HashMap<String, bool> = [
            ("session_restore".to_string(), true),
            ("precise_recall".to_string(), true),
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

        let mut result = self
            .build_prompt_context(
                user_id,
                query,
                &context,
                &section_policy,
                &section_intensity,
                user_emotion_label,
            )
            .await?;

        if !include_conversations {
            result.sections.remove("history_messages");
        }

        Ok(result)
    }

    /// 构建完整的提示词上下文
    pub async fn build_prompt_context(
        &self,
        user_id: &str,
        query: &str,
        context: &MemoryAccessContext,
        section_policy: &HashMap<String, bool>,
        section_intensity: &HashMap<String, String>,
        user_emotion_label: &str,
    ) -> XueliResult<PromptContextResult> {
        // 1. 获取重要记忆
        let important_memories = self.get_important_memories(user_id, context).await?;

        // 2. 获取普通记忆
        let ordinary_memories = self.load_accessible_ordinary_memories(context).await?;

        // 3. 按提示词分类构建各类条目
        let user_important: Vec<PromptEntry> = important_memories
            .iter()
            .filter(|(mem, _metadata)| {
                self.access_policy.classify_for_prompt(
                    None,
                    &mem.user_id,
                    &context.requester_user_id,
                ) == "private"
            })
            .map(|(mem, _)| {
                self.memory_to_prompt_entry(mem, "user_important", &context.requester_user_id)
            })
            .collect();

        let addressing: Vec<PromptEntry> = {
            let combined: Vec<&MemoryItem> = important_memories
                .iter()
                .map(|(m, _)| m)
                .chain(ordinary_memories.iter())
                .collect();
            combined
                .iter()
                .filter(|mem| {
                    self.access_policy.classify_for_prompt(
                        None,
                        &mem.user_id,
                        &context.requester_user_id,
                    ) == "addressing"
                })
                .map(|mem| {
                    self.memory_to_prompt_entry(mem, "addressing", &context.requester_user_id)
                })
                .collect()
        };

        let shared_memories: Vec<PromptEntry> = {
            let combined: Vec<&MemoryItem> = important_memories
                .iter()
                .map(|(m, _)| m)
                .chain(ordinary_memories.iter())
                .collect();
            combined
                .iter()
                .filter(|mem| {
                    self.access_policy.classify_for_prompt(
                        None,
                        &mem.user_id,
                        &context.requester_user_id,
                    ) == "shared"
                })
                .map(|mem| self.memory_to_prompt_entry(mem, "shared", &context.requester_user_id))
                .collect()
        };

        // 4. session restore / precise recall（集成对应服务）
        let session_restore: Vec<PromptEntry> = if section_policy
            .get("session_restore")
            .copied()
            .unwrap_or(true)
        {
            self.build_session_restore_entries(context).await
        } else {
            Vec::new()
        };

        let precise_recall: Vec<PromptEntry> = if section_policy
            .get("precise_recall")
            .copied()
            .unwrap_or(true)
        {
            self.build_precise_recall_entries(query, context).await
        } else {
            Vec::new()
        };

        // 5. 动态记忆条目
        let dynamic_memories = if section_policy.get("dynamic").copied().unwrap_or(true) {
            self.build_dynamic_memory_entries(
                query,
                user_id,
                context,
                &important_memories,
                &ordinary_memories,
                user_emotion_label,
            )
            .await?
        } else {
            Vec::new()
        };

        // 6. 去重
        let user_important = self.access_policy.dedupe_prompt_entries(&user_important);
        let addressing = self.access_policy.dedupe_prompt_entries(&addressing);
        let shared_memories = self.access_policy.dedupe_prompt_entries(&shared_memories);
        let session_restore = self.access_policy.dedupe_prompt_entries(&session_restore);
        let precise_recall = self.access_policy.dedupe_prompt_entries(&precise_recall);
        let dynamic_memories = self.access_policy.dedupe_prompt_entries(&dynamic_memories);

        // 7. 按预算裁剪
        let session_intensity = section_intensity
            .get("session_restore")
            .cloned()
            .unwrap_or_else(|| "normal".to_string());
        let precise_intensity = section_intensity
            .get("precise_recall")
            .cloned()
            .unwrap_or_else(|| "normal".to_string());
        let dynamic_intensity = section_intensity
            .get("dynamic")
            .cloned()
            .unwrap_or_else(|| "normal".to_string());

        let mut sections: HashMap<String, Vec<PromptEntry>> = HashMap::new();
        sections.insert(
            "user_important".to_string(),
            self.trim_entries(&user_important, self.prompt_budgets.user_important),
        );
        sections.insert(
            "addressing".to_string(),
            self.trim_entries(&addressing, self.prompt_budgets.addressing),
        );
        sections.insert(
            "shared".to_string(),
            self.trim_entries(&shared_memories, self.prompt_budgets.shared),
        );
        sections.insert(
            "session_restore".to_string(),
            self.trim_entries(
                &session_restore,
                self.budget_for_intensity("session_restore", &session_intensity),
            ),
        );
        sections.insert(
            "precise_recall".to_string(),
            self.trim_entries(
                &precise_recall,
                self.budget_for_intensity("precise_recall", &precise_intensity),
            ),
        );
        sections.insert(
            "dynamic".to_string(),
            self.trim_entries(
                &dynamic_memories,
                self.budget_for_intensity("dynamic", &dynamic_intensity),
            ),
        );

        // 8. 收集使用的记忆 ID
        let mut used_memory_ids: Vec<String> = Vec::new();
        for section_key in &["user_important", "addressing", "shared", "dynamic"] {
            if let Some(entries) = sections.get(*section_key) {
                for entry in entries {
                    if let Some(mid) = entry
                        .get("memory_id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        used_memory_ids.push(mid.to_string());
                    }
                }
            }
        }

        // 9. 构建提示词文本
        let prompt_text = self.build_prompt_text(&sections);

        Ok(PromptContextResult {
            prompt_text,
            session_restore: sections.remove("session_restore").unwrap_or_default(),
            precise_recall: sections.remove("precise_recall").unwrap_or_default(),
            dynamic_memories: sections.get("dynamic").cloned().unwrap_or_default(),
            sections,
            used_memory_ids,
        })
    }

    /// 搜索重要记忆（带评分排序）
    pub async fn search_important_memories(
        &self,
        user_id: &str,
        query: &str,
        _context: &MemoryAccessContext,
        limit: usize,
    ) -> XueliResult<Vec<PromptEntry>> {
        let important_store = match &self.important_store {
            Some(s) => s,
            None => return Ok(Vec::new()),
        };

        let mut entries: Vec<PromptEntry> = Vec::new();

        let imp_records = important_store.get_important(user_id, limit * 2).await?;
        for record in &imp_records {
            let memory_item = MemoryItem {
                id: record.id.clone(),
                user_id: record.user_id.clone(),
                content: record.content.clone(),
                memory_type: crate::core::types::MemoryType::Fact,
                importance: record.score,
                created_at: record.created_at,
                last_accessed_at: record.updated_at,
                access_count: record.recall_count as u64,
            };
            if !self
                .access_policy
                .is_accessible(&memory_item, &ChatScope::Private)
            {
                continue;
            }
            let mut entry = HashMap::new();
            entry.insert(
                "content".to_string(),
                serde_json::Value::String(record.content.clone()),
            );
            entry.insert(
                "source".to_string(),
                serde_json::Value::String("important".to_string()),
            );
            entry.insert(
                "priority".to_string(),
                serde_json::Value::Number(
                    serde_json::Number::from_f64(record.score).unwrap_or(0.into()),
                ),
            );
            entry.insert(
                "score".to_string(),
                serde_json::Value::Number(
                    serde_json::Number::from_f64(self.score_text(query, &record.content))
                        .unwrap_or(0.into()),
                ),
            );
            entry.insert(
                "memory_type".to_string(),
                serde_json::Value::String("important".to_string()),
            );
            entry.insert(
                "memory_owner".to_string(),
                serde_json::Value::String(record.user_id.clone()),
            );
            entries.push(entry);
        }

        entries.sort_by(|a, b| {
            let score_a = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let score_b = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let pri_a = a.get("priority").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let pri_b = b.get("priority").and_then(|v| v.as_f64()).unwrap_or(0.0);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    pri_b
                        .partial_cmp(&pri_a)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        entries.truncate(limit);
        Ok(entries)
    }

    /// 格式化重要记忆为提示词注入文本
    pub async fn format_important_memories_for_prompt(
        &self,
        user_id: &str,
        context: &MemoryAccessContext,
        limit: usize,
    ) -> XueliResult<String> {
        let important_memories = self.get_important_memories(user_id, context).await?;
        if important_memories.is_empty() {
            return Ok(String::new());
        }

        let limited: Vec<_> = important_memories.into_iter().take(limit).collect();
        let mut lines = vec!["=== 重要事实（请务必记住） ===".to_string()];

        for (index, (memory, _)) in limited.iter().enumerate() {
            let num = index + 1;
            if memory.user_id != user_id && !memory.user_id.is_empty() {
                lines.push(format!(
                    "{}. [来源用户 {}] {}",
                    num, memory.user_id, memory.content
                ));
            } else {
                lines.push(format!("{}. {}", num, memory.content));
            }
        }
        lines.push(String::new());
        Ok(lines.join("\n"))
    }

    // ── 内部方法 ──────────

    /// 获取用户的重要记忆（带访问策略过滤）
    async fn get_important_memories(
        &self,
        user_id: &str,
        _context: &MemoryAccessContext,
    ) -> XueliResult<Vec<(MemoryItem, Option<serde_json::Value>)>> {
        let important_store = match &self.important_store {
            Some(s) => s,
            None => return Ok(Vec::new()),
        };

        let mut results: Vec<(MemoryItem, Option<serde_json::Value>)> = Vec::new();
        let records = important_store.get_important(user_id, 12).await?;

        for record in records {
            let memory_item = MemoryItem {
                id: record.id.clone(),
                user_id: record.user_id.clone(),
                content: record.content.clone(),
                memory_type: crate::core::types::MemoryType::Fact,
                importance: record.score,
                created_at: record.created_at,
                last_accessed_at: record.updated_at,
                access_count: record.recall_count as u64,
            };
            if self
                .access_policy
                .is_accessible(&memory_item, &ChatScope::Private)
            {
                results.push((memory_item, None));
            }
        }

        Ok(results)
    }

    /// 加载可访问的普通记忆
    async fn load_accessible_ordinary_memories(
        &self,
        context: &MemoryAccessContext,
    ) -> XueliResult<Vec<MemoryItem>> {
        let all_memories = self.store.get_by_user(&context.requester_user_id).await?;
        let accessible: Vec<MemoryItem> = all_memories
            .into_iter()
            .filter(|m| self.access_policy.is_accessible(m, &ChatScope::Private))
            .collect();
        Ok(accessible)
    }

    /// 构建会话恢复条目（session restore）
    async fn build_session_restore_entries(
        &self,
        context: &MemoryAccessContext,
    ) -> Vec<PromptEntry> {
        let conversation_store = match &self.conversation_store {
            Some(s) => s,
            None => return Vec::new(),
        };

        let recent_limit = 12; // recent_session_limit
        let entry_limit = 2; // restore_entry_limit

        // 构建 dialogue_key
        let is_group = context.message_type == "group" && !context.group_id.is_empty();
        let dialogue_key = if is_group {
            format!(
                "{}:{}:{}",
                context.platform, context.message_type, context.group_id
            )
        } else {
            format!("{}:private:{}", context.platform, context.requester_user_id)
        };

        // 获取最近消息
        let recent_messages = if is_group {
            match conversation_store
                .get_recent_by_scope("group", &context.group_id, recent_limit * 20)
                .await
            {
                Ok(msgs) => msgs,
                Err(_) => return Vec::new(),
            }
        } else {
            match conversation_store
                .get_recent_by_user(&context.requester_user_id, recent_limit * 20)
                .await
            {
                Ok(msgs) => msgs,
                Err(_) => return Vec::new(),
            }
        };

        // 按 session_id 分组
        let mut sessions: HashMap<String, Vec<ConversationRecord>> = HashMap::new();
        for msg in &recent_messages {
            let sid = if msg.session_id.is_empty() {
                dialogue_key.clone()
            } else {
                msg.session_id.clone()
            };
            sessions.entry(sid).or_default().push(msg.clone());
        }
        for msgs in sessions.values_mut() {
            msgs.sort_by(|a, b| {
                a.event_time
                    .partial_cmp(&b.event_time)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // 按最后消息时间排序 session
        let mut sorted_sessions: Vec<_> = sessions.into_iter().collect();
        sorted_sessions.sort_by(|(_, a), (_, b)| {
            let ta = a.last().map(|m| m.event_time).unwrap_or(0.0);
            let tb = b.last().map(|m| m.event_time).unwrap_or(0.0);
            tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut entries: Vec<PromptEntry> = Vec::new();
        let mut count = 0;

        for (_session_id, messages) in &sorted_sessions {
            if count >= entry_limit {
                break;
            }
            if messages.is_empty() {
                continue;
            }

            let turn_count = messages.len() as i64;
            let text_msgs: Vec<String> = messages.iter().map(|m| m.text.clone()).collect();
            let summary =
                ChatSummaryService::<crate::services::ai_client::DefaultAIClient>::summarize_simple(
                    &text_msgs,
                );

            let last_msg = messages.last().unwrap();
            let closed_at = format!("{:.0}", last_msg.event_time);

            let label = if count == 0 {
                "上一轮会话"
            } else {
                "更早一轮会话"
            };
            let t = if !closed_at.is_empty() {
                format!("（{}，{}轮）", closed_at, turn_count)
            } else {
                format!("（{}轮）", turn_count)
            };
            let content = format!("{}{}：{}", label, t, summary);

            let mut entry = HashMap::new();
            entry.insert("content".to_string(), serde_json::Value::String(content));
            entry.insert(
                "source".to_string(),
                serde_json::Value::String("session_restore".to_string()),
            );
            entry.insert(
                "score".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(0.5).unwrap()),
            );
            entries.push(entry);
            count += 1;
        }

        entries
    }

    /// 构建精准回忆条目（precise recall）
    async fn build_precise_recall_entries(
        &self,
        query: &str,
        context: &MemoryAccessContext,
    ) -> Vec<PromptEntry> {
        let recall_service = match &self.recall_service {
            Some(s) => s,
            None => return Vec::new(),
        };

        let recalled = match recall_service
            .recall(&context.requester_user_id, "", query)
            .await
        {
            Ok(items) => items,
            Err(_) => return Vec::new(),
        };

        recalled
            .into_iter()
            .enumerate()
            .map(|(idx, item)| {
                let label = if idx == 0 {
                    "与当前话题相关的历史"
                } else {
                    "前述话题的延续"
                };
                let content = format!("{}：{}", label, item.content);
                let mut entry = HashMap::new();
                entry.insert("content".to_string(), serde_json::Value::String(content));
                entry.insert(
                    "source".to_string(),
                    serde_json::Value::String("precise_recall".to_string()),
                );
                entry.insert(
                    "score".to_string(),
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(item.importance).unwrap_or(0.into()),
                    ),
                );
                entry
            })
            .collect()
    }

    /// 构建动态记忆条目（混合检索 + 情绪加权 + 场景分桶）
    async fn build_dynamic_memory_entries(
        &self,
        query: &str,
        user_id: &str,
        context: &MemoryAccessContext,
        _important_memories: &[(MemoryItem, Option<serde_json::Value>)],
        ordinary_memories: &[MemoryItem],
        user_emotion_label: &str,
    ) -> XueliResult<Vec<PromptEntry>> {
        let dynamic_limit = 8;
        let mut scored: Vec<(f64, PromptEntry)> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();

        // 先尝试通过混合检索获取结果
        let retrieval_result = self
            .retrieve(query, user_id, dynamic_limit * 2, dynamic_limit * 2)
            .await?;

        for (i, item) in retrieval_result.items.iter().enumerate() {
            if seen_ids.contains(&item.id) {
                continue;
            }
            if self.should_skip_dynamic_memory(item, query) {
                continue;
            }
            if self.access_policy.is_addressing(None) {
                continue;
            }

            let entry = self.memory_to_prompt_entry(item, "dynamic", user_id);
            let score = retrieval_result.scores.get(i).copied().unwrap_or(0.0);
            let mut entry = entry;
            entry.insert(
                "score".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(score).unwrap_or(0.into())),
            );
            entry.insert(
                "bm25_score".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(score).unwrap_or(0.into())),
            );

            scored.push((score, entry));
            seen_ids.insert(item.id.clone());

            if scored.len() >= dynamic_limit {
                break;
            }
        }

        // 回退：对普通记忆做文本相关性评分
        if scored.is_empty() {
            for memory in ordinary_memories {
                let score = self.score_text(query, &memory.content);
                if score <= 0.0 {
                    continue;
                }
                if self.should_skip_dynamic_memory(memory, query) {
                    continue;
                }
                if self.access_policy.is_addressing(None) {
                    continue;
                }

                let mut entry = self.memory_to_prompt_entry(memory, "dynamic", user_id);
                entry.insert(
                    "score".to_string(),
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(score).unwrap_or(0.into()),
                    ),
                );
                scored.push((score, entry));

                if scored.len() >= dynamic_limit {
                    break;
                }
            }
        }

        // 情绪加权
        if !scored.is_empty() && !user_emotion_label.is_empty() {
            self.apply_emotion_boost(&mut scored, user_emotion_label);
        }

        // 按场景分桶 + 分数排序
        scored.sort_by(|a, b| {
            let bucket_a = self.dynamic_scene_bucket(&a.1, context);
            let bucket_b = self.dynamic_scene_bucket(&b.1, context);
            bucket_b
                .cmp(&bucket_a)
                .then_with(|| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal))
        });

        // 去重
        let deduped = self.dedupe_dynamic_entries(&scored, context, dynamic_limit);

        Ok(deduped)
    }

    // ── 辅助计算函数 ──────────

    /// 简单文本相关性评分（基于字符重叠和子串匹配）
    fn score_text(&self, query: &str, content: &str) -> f64 {
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

        // 子串匹配
        if nc.contains(&nq) || nq.contains(&nc) {
            return nq.len().min(nc.len()) as f64 / nq.len().max(nc.len()) as f64;
        }

        // 字符重叠
        let q_chars: HashSet<char> = nq.chars().collect();
        let c_chars: HashSet<char> = nc.chars().collect();
        let overlap = q_chars.intersection(&c_chars).count();
        overlap as f64 / q_chars.len().max(1) as f64
    }

    /// 判断是否跳过某条动态记忆
    fn should_skip_dynamic_memory(&self, memory: &MemoryItem, query: &str) -> bool {
        let content = memory.content.trim();
        if content.len() < 3 {
            return true;
        }
        self.score_text(query, content) < 0.12
    }

    /// 情绪加权
    fn apply_emotion_boost(&self, scored: &mut [(f64, PromptEntry)], user_emotion_label: &str) {
        if user_emotion_label.is_empty() {
            return;
        }
        for (score, entry) in scored.iter_mut() {
            // 从 metadata 中读取情感的 tone（当前版本 metadata 可能为空）
            let metadata = entry.get("metadata").and_then(|v| v.as_object());
            let mem_tone = metadata
                .and_then(|m| m.get("emotional_tone"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if mem_tone.is_empty() {
                continue;
            }
            if mem_tone == user_emotion_label {
                *score *= 1.1;
            } else if Self::emotion_is_complementary(user_emotion_label, mem_tone) {
                *score *= 1.05;
            }
        }
    }

    /// 判断两个情绪标签是否互补
    fn emotion_is_complementary(a: &str, b: &str) -> bool {
        let pairs: &[(&str, &str)] = &[
            ("伤心", "委屈"),
            ("委屈", "伤心"),
            ("害怕", "困惑"),
            ("困惑", "害怕"),
            ("开心", "喜欢"),
            ("喜欢", "开心"),
            ("生气", "无语"),
            ("无语", "生气"),
        ];
        pairs.iter().any(|(x, y)| *x == a && *y == b)
    }

    /// 场景相关性分桶（越高越优先展示）
    fn dynamic_scene_bucket(&self, entry: &PromptEntry, context: &MemoryAccessContext) -> i32 {
        let metadata = entry.get("metadata").and_then(|v| v.as_object());
        let owner_user_id = entry
            .get("owner_user_id")
            .or(entry.get("memory_owner"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let source_message_type = metadata
            .and_then(|m| m.get("source_message_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let source_group_id = metadata
            .and_then(|m| m.get("source_group_id").or(m.get("group_id")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        let same_group = context.message_type == "group"
            && !context.group_id.is_empty()
            && source_group_id == context.group_id;

        let same_message_type =
            !source_message_type.is_empty() && source_message_type == context.message_type;

        let same_owner =
            !context.requester_user_id.is_empty() && owner_user_id == context.requester_user_id;

        if same_group && same_owner {
            4
        } else if same_group {
            3
        } else if same_message_type && same_owner {
            2
        } else if same_message_type {
            1
        } else {
            0
        }
    }

    /// 动态条目去重（基于内容相似度）
    fn dedupe_dynamic_entries(
        &self,
        scored: &[(f64, PromptEntry)],
        context: &MemoryAccessContext,
        limit: usize,
    ) -> Vec<PromptEntry> {
        if !self.dynamic_dedup_enabled {
            return scored
                .iter()
                .take(limit)
                .map(|(_, entry)| entry.clone())
                .collect();
        }
        let threshold = 0.72;
        let mut accepted: Vec<PromptEntry> = Vec::new();

        for (_, entry) in scored.iter() {
            if self.is_duplicate_dynamic_entry(entry, &accepted, threshold) {
                continue;
            }
            accepted.push(entry.clone());
            if accepted.len() >= limit {
                break;
            }
        }

        accepted.sort_by(|a, b| {
            let bucket_a = self.dynamic_scene_bucket(a, context);
            let bucket_b = self.dynamic_scene_bucket(b, context);
            bucket_b.cmp(&bucket_a).then_with(|| {
                let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        accepted
    }

    /// 判断动态条目是否与已接受条目重复
    fn is_duplicate_dynamic_entry(
        &self,
        entry: &PromptEntry,
        accepted: &[PromptEntry],
        threshold: f64,
    ) -> bool {
        let content = self
            .normalize_dynamic_text(entry.get("content").and_then(|v| v.as_str()).unwrap_or(""));
        if content.is_empty() {
            return false;
        }

        for acc in accepted {
            let acc_content = self
                .normalize_dynamic_text(acc.get("content").and_then(|v| v.as_str()).unwrap_or(""));
            if acc_content.is_empty() {
                continue;
            }
            if content == acc_content {
                return true;
            }
            if content.contains(&acc_content) || acc_content.contains(&content) {
                return true;
            }
            if self.dynamic_text_similarity(&content, &acc_content) >= threshold {
                return true;
            }
        }
        false
    }

    /// 规范化动态记忆文本（用于去重比较）
    fn normalize_dynamic_text(&self, text: &str) -> String {
        let lower = text.to_lowercase();
        let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
        // 去除标点后若为空，返回去空白版本
        let normalized: String = compact
            .chars()
            .filter(|c| c.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
            .collect();
        if normalized.is_empty() {
            compact
        } else {
            normalized
        }
    }

    /// 文本相似度（基于 bigram 重叠系数）
    fn dynamic_text_similarity(&self, left: &str, right: &str) -> f64 {
        if left.is_empty() || right.is_empty() {
            return 0.0;
        }

        let left_bigrams = self.to_character_bigrams(left);
        let right_bigrams = self.to_character_bigrams(right);
        let bigram_score = overlap_coefficient(&left_bigrams, &right_bigrams);
        if bigram_score > 0.0 {
            return bigram_score;
        }

        if left == right {
            return 1.0;
        }
        if left.contains(right) || right.contains(left) {
            return left.len().min(right.len()) as f64 / left.len().max(right.len()) as f64;
        }
        0.0
    }

    /// 转为字符 bigram 集合
    fn to_character_bigrams(&self, text: &str) -> HashSet<String> {
        let compact: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.len() < 2 {
            return HashSet::new();
        }
        compact
            .windows(2)
            .map(|w| w.iter().collect::<String>())
            .collect()
    }

    /// 将 MemoryItem 转为提示词条目
    fn memory_to_prompt_entry(
        &self,
        memory: &MemoryItem,
        section: &str,
        requester_user_id: &str,
    ) -> PromptEntry {
        let mut content = memory.content.clone();
        let label_owner = memory.user_id != requester_user_id && !memory.user_id.is_empty();

        if section == "shared" && label_owner {
            content = format!("[来源用户 {}] {}", memory.user_id, content);
        }

        let mut entry = HashMap::new();
        entry.insert("content".to_string(), serde_json::Value::String(content));
        entry.insert(
            "memory_id".to_string(),
            serde_json::Value::String(memory.id.clone()),
        );
        entry.insert(
            "memory_owner".to_string(),
            serde_json::Value::String(memory.user_id.clone()),
        );
        entry.insert(
            "owner_user_id".to_string(),
            serde_json::Value::String(memory.user_id.clone()),
        );
        entry.insert(
            "score".to_string(),
            serde_json::Value::Number(
                serde_json::Number::from_f64(memory.importance).unwrap_or(0.into()),
            ),
        );
        entry
    }

    /// 按字符预算裁剪条目
    fn trim_entries(&self, entries: &[PromptEntry], budget: usize) -> Vec<PromptEntry> {
        let mut trimmed: Vec<PromptEntry> = Vec::new();
        let mut used = 0usize;

        for entry in entries {
            let content = entry
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if content.is_empty() {
                continue;
            }
            let len = content.chars().count();
            if !trimmed.is_empty() && used + len > budget {
                break;
            }
            trimmed.push(entry.clone());
            used += len;
        }
        trimmed
    }

    /// 根据强度等级调整预算
    fn budget_for_intensity(&self, section: &str, intensity: &str) -> usize {
        let base = match section {
            "session_restore" => self.prompt_budgets.session_restore,
            "precise_recall" => self.prompt_budgets.precise_recall,
            "dynamic" => self.prompt_budgets.dynamic,
            _ => 0,
        };
        let multiplier = match intensity {
            "off" => 0.0,
            "light" => 0.6,
            "normal" => 1.0,
            "high" => 1.4,
            _ => 1.0,
        };
        (base as f64 * multiplier).max(0.0) as usize
    }

    /// 构建提示词文本
    fn build_prompt_text(&self, sections: &HashMap<String, Vec<PromptEntry>>) -> String {
        let mut parts: Vec<String> = Vec::new();

        let section_specs: Vec<(&str, &str)> = vec![
            ("=== 当前用户重要记忆 ===", "user_important"),
            ("=== 当前场景称呼要求 ===", "addressing"),
            ("=== 当前场景共享规则 / 共享重要记忆 ===", "shared"),
            ("=== 最近相关会话恢复 ===", "session_restore"),
            ("=== 相关旧对话精准定位 ===", "precise_recall"),
        ];

        for (title, key) in &section_specs {
            let entries = sections.get(*key);
            if entries.is_none() || entries.unwrap().is_empty() {
                continue;
            }
            let entries = entries.unwrap();
            parts.push(title.to_string());
            for (idx, entry) in entries.iter().enumerate() {
                if let Some(content) = entry.get("content").and_then(|v| v.as_str()) {
                    let content = if *key == "precise_recall" {
                        let metadata = entry.get("metadata").and_then(|v| v.as_object());
                        let confidence = metadata
                            .and_then(|m| m.get("recall_confidence"))
                            .and_then(|v| v.as_f64());
                        self.recall_renderer
                            .as_ref()
                            .map(|r| r.render_entry(content, confidence))
                            .unwrap_or_else(|| content.to_string())
                    } else {
                        content.to_string()
                    };
                    parts.push(format!("{}. {}", idx + 1, content));
                }
            }
            parts.push(String::new());
        }

        if let Some(dynamic) = sections.get("dynamic") {
            if !dynamic.is_empty() {
                parts.push(
                    "=== 相关过往信息（请用你自己的话自然融入回复，不要逐字引用） ===".to_string(),
                );
                for (idx, entry) in dynamic.iter().enumerate() {
                    if let Some(content) = entry.get("content").and_then(|v| v.as_str()) {
                        parts.push(format!("{}. {}", idx + 1, content));
                    }
                }
                parts.push(String::new());
            }
        }

        let text = parts.join("\n").trim().to_string();

        // 对精准回忆 section 应用模糊回忆渲染（若启用）
        self.apply_recall_renderer_to_text(&text)
    }

    /// 在精准回忆 section 前插入模糊回忆指导
    fn apply_recall_renderer_to_text(&self, prompt_text: &str) -> String {
        let renderer = match &self.recall_renderer {
            Some(r) if r.enabled => r,
            _ => return prompt_text.to_string(),
        };
        let precise_marker = "=== 相关旧对话精准定位 ===";
        if !prompt_text.contains(precise_marker) {
            return prompt_text.to_string();
        }
        let instruction = renderer.render_fuzzy_instruction();
        if instruction.is_empty() {
            return prompt_text.to_string();
        }
        prompt_text.replace(
            precise_marker,
            &format!("{}\n\n{}", instruction, precise_marker),
        )
    }
}

/// bigram 重叠系数
fn overlap_coefficient(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(right).count();
    intersection as f64 / left.len().min(right.len()) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::MemoryType;
    use crate::memory::stores::memory_item::SqliteMemoryItemStore;
    use chrono::Utc;

    fn make_item(id: &str, user_id: &str, content: &str) -> MemoryItem {
        MemoryItem {
            id: id.to_string(),
            user_id: user_id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Fact,
            importance: 0.5,
            created_at: Utc::now(),
            last_accessed_at: Utc::now(),
            access_count: 0,
        }
    }

    #[tokio::test]
    async fn test_index_and_retrieve() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());

        let items = vec![
            make_item("m1", "u1", "用户喜欢喝咖啡，每天早晨都要来一杯"),
            make_item("m2", "u1", "用户住在北京朝阳区"),
            make_item("m3", "u1", "用户是后端开发工程师"),
        ];

        for item in &items {
            store.store(item.clone()).await.unwrap();
        }

        let coordinator = RetrievalCoordinator::new(store.clone());
        coordinator.index_items(&items).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let result = coordinator.retrieve("咖啡", "u1", 5, 0).await.unwrap();
        assert!(
            !result.items.is_empty(),
            "Expected non-empty results for '咖啡'"
        );
        let ids: Vec<&str> = result.items.iter().map(|m| m.id.as_str()).collect();
        assert!(
            ids.contains(&"m1"),
            "Expected m1 (about 咖啡) in results, got: {ids:?}"
        );
    }

    #[tokio::test]
    async fn test_retrieve_user_filter() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());

        let items = vec![
            make_item("m1", "u1", "喜欢咖啡"),
            make_item("m2", "u2", "喜欢咖啡"),
        ];

        for item in &items {
            store.store(item.clone()).await.unwrap();
        }

        let coordinator = RetrievalCoordinator::new(store.clone());
        coordinator.index_items(&items).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let result = coordinator.retrieve("咖啡", "u1", 5, 0).await.unwrap();
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].user_id, "u1");
    }

    #[tokio::test]
    async fn test_score_text() {
        let store =
            Arc::new(SqliteMemoryItemStore::new(tempfile::tempdir().unwrap().path()).unwrap());
        let coordinator = RetrievalCoordinator::new(store);

        let score = coordinator.score_text("咖啡", "用户喜欢喝咖啡，每天早晨一杯");
        assert!(score > 0.0);

        let score_zero = coordinator.score_text("篮球", "用户喜欢喝咖啡");
        assert!(score_zero < 0.12);
    }

    #[tokio::test]
    async fn test_build_prompt_context() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());

        let items = vec![
            make_item("m1", "u1", "用户喜欢喝咖啡"),
            make_item("m2", "u1", "用户住在北京"),
        ];
        for item in &items {
            store.store(item.clone()).await.unwrap();
        }

        let coordinator = RetrievalCoordinator::new(store.clone());
        coordinator.index_items(&items).await;

        let context = MemoryAccessContext::new("u1", &ChatScope::Private);
        let policy = HashMap::new();
        let intensity: HashMap<String, String> = [
            ("session_restore".to_string(), "normal".to_string()),
            ("precise_recall".to_string(), "normal".to_string()),
            ("dynamic".to_string(), "normal".to_string()),
        ]
        .into_iter()
        .collect();

        let result = coordinator
            .build_prompt_context("u1", "咖啡", &context, &policy, &intensity, "")
            .await
            .unwrap();

        // 应该有动态记忆条目返回
        assert!(!result.dynamic_memories.is_empty());
        // prompt text 应该包含记忆内容
        assert!(result.prompt_text.contains("用户喜欢喝咖啡"));
    }

    #[tokio::test]
    async fn test_emotion_is_complementary() {
        assert!(RetrievalCoordinator::emotion_is_complementary(
            "伤心", "委屈"
        ));
        assert!(RetrievalCoordinator::emotion_is_complementary(
            "委屈", "伤心"
        ));
        assert!(!RetrievalCoordinator::emotion_is_complementary(
            "伤心", "开心"
        ));
    }

    #[test]
    fn test_normalize_dynamic_text() {
        let store =
            Arc::new(SqliteMemoryItemStore::new(tempfile::tempdir().unwrap().path()).unwrap());
        let coordinator = RetrievalCoordinator::new(store);

        let norm = coordinator.normalize_dynamic_text("用户喜欢喝 咖啡！");
        assert!(
            norm.contains("用户喜欢喝咖啡")
                || norm.contains("用户喜欢喝咖啡！".to_lowercase().as_str())
        );
    }

    #[test]
    fn test_dynamic_text_similarity() {
        let store =
            Arc::new(SqliteMemoryItemStore::new(tempfile::tempdir().unwrap().path()).unwrap());
        let coordinator = RetrievalCoordinator::new(store);

        let sim = coordinator.dynamic_text_similarity("用户喜欢喝咖啡", "用户喜欢喝咖啡");
        assert!((sim - 1.0).abs() < 0.001);

        let sim2 = coordinator.dynamic_text_similarity("用户喜欢咖啡", "用户讨厌咖啡");
        assert!(sim2 < 1.0 && sim2 > 0.0);
    }

    #[test]
    fn test_prompt_budgets_default() {
        let budgets = PromptBudgets::default();
        assert_eq!(budgets.user_important, 360);
        assert_eq!(budgets.addressing, 180);
        assert_eq!(budgets.shared, 260);
        assert_eq!(budgets.dynamic, 420);
    }

    #[test]
    fn test_memory_access_context_new() {
        let ctx = MemoryAccessContext::new("user1", &ChatScope::Private);
        assert_eq!(ctx.requester_user_id, "user1");
        assert_eq!(ctx.message_type, "private");
        assert!(ctx.group_id.is_empty());

        let ctx = MemoryAccessContext::new("user1", &ChatScope::Group("g1".into()));
        assert_eq!(ctx.message_type, "group");
        assert_eq!(ctx.group_id, "g1");
    }

    #[test]
    fn test_budget_for_intensity() {
        let budgets = PromptBudgets::default();
        let store =
            Arc::new(SqliteMemoryItemStore::new(tempfile::tempdir().unwrap().path()).unwrap());
        let coordinator = RetrievalCoordinator::new(store).with_prompt_budgets(budgets.clone());

        let normal = coordinator.budget_for_intensity("dynamic", "normal");
        assert_eq!(normal, 420);

        let high = coordinator.budget_for_intensity("dynamic", "high");
        assert_eq!(high, 588);

        let off = coordinator.budget_for_intensity("dynamic", "off");
        assert_eq!(off, 0);
    }

    #[tokio::test]
    async fn test_format_important_memories_for_prompt_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());
        let coordinator = RetrievalCoordinator::new(store);
        let context = MemoryAccessContext::new("u1", &ChatScope::Private);

        let result = coordinator
            .format_important_memories_for_prompt("u1", &context, 5)
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_clear_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());

        let item = make_item("m1", "u1", "测试内容");
        store.store(item.clone()).await.unwrap();

        let coordinator = RetrievalCoordinator::new(store.clone());
        coordinator.index_item(&item, "doc_0").await;

        let result = coordinator.retrieve("测试", "u1", 5, 0).await.unwrap();
        assert!(!result.items.is_empty());

        coordinator.clear_index().await;

        let result = coordinator.retrieve("测试", "u1", 5, 0).await.unwrap();
        assert!(result.items.is_empty());
    }
}
