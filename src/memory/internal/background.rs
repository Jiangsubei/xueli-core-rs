use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use tokio::time::{interval, Duration};

use crate::core::config::MemoryConfig;
use crate::core::types::{MemoryItem, MemoryPatch, MemoryType};
use crate::memory::chat_summary_service::ChatSummaryService;
use crate::memory::extraction::buffer::{BufferTurn, ExtractionBuffer};
use crate::memory::internal::task_manager::MemoryTaskManager;
use crate::memory::person_fact_service::PersonFactService;
use crate::memory::stores::conversation::{MessageRecord, SqliteConversationStore};
use crate::memory::stores::important::ImportantMemoryStore;
use crate::memory::stores::memory_item::SqliteMemoryItemStore;
use crate::memory::stores::traits::MemoryStore;
use crate::prelude::XueliResult;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};
use crate::traits::prompt_template::PromptTemplateLoader;

/// 巩固输入 — 用于 LLM 评估记忆半衰期调制器
#[derive(Debug, Clone)]
pub struct ConsolidationInput {
    pub id: String,
    pub content: String,
    pub importance: f64,
    pub mention_count: i64,
    pub recall_count: i64,
    pub emotional_tone: String,
    pub category: String,
}

pub struct MemoryBackgroundCoordinator<L: PromptTemplateLoader + 'static> {
    config: Arc<MemoryConfig>,

    conversation_store: Option<Arc<SqliteConversationStore>>,
    extractor_buffer: Arc<Mutex<ExtractionBuffer>>,
    task_manager: Arc<MemoryTaskManager>,
    memory_store: Option<Arc<SqliteMemoryItemStore>>,
    important_store: Option<Arc<ImportantMemoryStore>>,
    person_fact_service: Option<Arc<PersonFactService>>,
    summary_service: Option<Arc<ChatSummaryService>>,
    llm_client: Option<Arc<dyn AIClient>>,
    prompt_loader: Arc<L>,

    on_digest_tick: Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>,
    on_memory_changed: Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>,
    on_insight_generated: Arc<Mutex<Option<Box<dyn Fn(String) + Send + Sync>>>>,

    running: Arc<AtomicBool>,
    digest_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,

    auto_extract_memory: bool,
    consolidation_enabled: bool,
    merge_enabled: bool,
    merge_min_cluster_size: usize,
    merge_max_batch: usize,
    consolidation_batch_size: usize,
    consolidation_hours: f64,
    llm_model: String,

    self_weak: Weak<Self>,
}

impl<L: PromptTemplateLoader + 'static> MemoryBackgroundCoordinator<L> {
    pub fn new(
        config: Arc<MemoryConfig>,
        task_manager: Arc<MemoryTaskManager>,
        prompt_loader: Arc<L>,
    ) -> Self {
        Self {
            auto_extract_memory: config.auto_extract,
            config,
            conversation_store: None,
            extractor_buffer: Arc::new(Mutex::new(ExtractionBuffer::new())),
            task_manager,
            memory_store: None,
            important_store: None,
            person_fact_service: None,
            summary_service: None,
            llm_client: None,
            prompt_loader,
            on_digest_tick: Arc::new(Mutex::new(None)),
            on_memory_changed: Arc::new(Mutex::new(None)),
            on_insight_generated: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            digest_handle: Mutex::new(None),
            consolidation_enabled: false,
            merge_enabled: false,
            merge_min_cluster_size: 2,
            merge_max_batch: 5,
            consolidation_batch_size: 20,
            consolidation_hours: 48.0,
            llm_model: "gpt-4o-mini".to_string(),
            self_weak: Weak::new(),
        }
    }

    pub fn into_arc(self) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            self_weak: weak.clone(),
            ..self
        })
    }

    pub fn with_conversation_store(mut self, store: Arc<SqliteConversationStore>) -> Self {
        self.conversation_store = Some(store);
        self
    }

    pub fn with_memory_store(mut self, store: Arc<SqliteMemoryItemStore>) -> Self {
        self.memory_store = Some(store);
        self
    }

    pub fn with_important_store(mut self, store: Arc<ImportantMemoryStore>) -> Self {
        self.important_store = Some(store);
        self
    }

    pub fn with_person_fact_service(mut self, service: Arc<PersonFactService>) -> Self {
        self.person_fact_service = Some(service);
        self
    }

    pub fn with_summary_service(mut self, service: Arc<ChatSummaryService>) -> Self {
        self.summary_service = Some(service);
        self
    }

    pub fn with_llm_client(mut self, client: Arc<dyn AIClient>) -> Self {
        self.llm_client = Some(client);
        self
    }

    pub fn with_llm_model(mut self, model: String) -> Self {
        self.llm_model = model;
        self
    }

    pub fn with_auto_extract(mut self, enabled: bool) -> Self {
        self.auto_extract_memory = enabled;
        self
    }

    pub fn with_consolidation_enabled(mut self, enabled: bool) -> Self {
        self.consolidation_enabled = enabled;
        self
    }

    pub fn with_consolidation_batch_size(mut self, size: usize) -> Self {
        self.consolidation_batch_size = size;
        self
    }

    pub fn with_consolidation_hours(mut self, hours: f64) -> Self {
        self.consolidation_hours = hours;
        self
    }

    pub fn with_merge_enabled(mut self, enabled: bool) -> Self {
        self.merge_enabled = enabled;
        self
    }

    pub fn with_merge_min_cluster_size(mut self, size: usize) -> Self {
        self.merge_min_cluster_size = size;
        self
    }

    pub fn with_merge_max_batch(mut self, batch: usize) -> Self {
        self.merge_max_batch = batch;
        self
    }

    pub fn set_digest_tick_callback<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        if let Ok(mut cb) = self.on_digest_tick.lock() {
            *cb = Some(Box::new(callback));
        }
    }

    pub fn set_memory_changed_callback<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        if let Ok(mut cb) = self.on_memory_changed.lock() {
            *cb = Some(Box::new(callback));
        }
    }

    pub fn set_insight_callback<F>(&self, callback: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        if let Ok(mut cb) = self.on_insight_generated.lock() {
            *cb = Some(Box::new(callback));
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn fire_memory_changed(&self) {
        if let Ok(cb) = self.on_memory_changed.lock() {
            if let Some(ref f) = *cb {
                f();
            }
        }
    }

    fn fire_insight(&self, insight: String) {
        if let Ok(cb) = self.on_insight_generated.lock() {
            if let Some(ref f) = *cb {
                f(insight);
            }
        }
    }

    fn fire_digest_tick(&self) {
        if let Ok(cb) = self.on_digest_tick.lock() {
            if let Some(ref f) = *cb {
                f();
            }
        }
    }

    fn weak_self(&self) -> Weak<Self> {
        self.self_weak.clone()
    }

    // ── Lifecycle ─────────────────────────────────────────────────

    pub fn start(&self, digest_interval_secs: u64) {
        if self.running.swap(true, Ordering::SeqCst) {
            tracing::debug!("[后台协调] 已在运行中，跳过重复启动");
            return;
        }

        let running = self.running.clone();
        let weak = self.weak_self();
        let interval_secs = digest_interval_secs.max(1);

        let handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_secs));
            ticker.tick().await;

            tracing::info!("[后台协调] 消化循环已启动，间隔 {} 秒", interval_secs);

            loop {
                ticker.tick().await;
                if !running.load(Ordering::SeqCst) {
                    tracing::info!("[后台协调] 消化循环已停止");
                    break;
                }

                if let Some(coord) = weak.upgrade() {
                    coord.fire_digest_tick();
                    if let Err(e) = coord.run_digestion_cycle().await {
                        tracing::warn!("[后台协调] 消化循环异常: {}", e);
                    }
                } else {
                    tracing::debug!("[后台协调] 协调器已释放，停止消化循环");
                    break;
                }

                tracing::debug!("[后台协调] 消化 tick 完成");
            }
        });

        if let Ok(mut guard) = self.digest_handle.lock() {
            *guard = Some(handle);
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Ok(mut guard) = self.digest_handle.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
        tracing::info!("[后台协调] 消化循环已停止");
    }

    // ── Dialogue Turn Registration ──────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub async fn register_dialogue_turn(
        &self,
        user_id: &str,
        user_message: &str,
        assistant_message: &str,
        session_id: &str,
        turn_id: usize,
        dialogue_key: &str,
        message_type: &str,
        group_id: &str,
        message_id: &str,
        narrative_summary: &str,
        source_platform: &str,
    ) {
        let registration = if let Some(ref store) = self.conversation_store {
            let timestamp = chrono::Utc::now().timestamp();
            let user_msg =
                MessageRecord::user(user_id, user_id, user_message, timestamp, message_id);
            let assistant_msg = MessageRecord::assistant(assistant_message, timestamp);
            match store.add_turn(session_id, &user_msg, &assistant_msg).await {
                Ok(reg) => Some(reg),
                Err(e) => {
                    tracing::warn!("[后台协调] 添加对话轮次失败: {}", e);
                    None
                }
            }
        } else {
            None
        };

        if let Ok(mut buf) = self.extractor_buffer.lock() {
            let resolved_session_id = registration
                .as_ref()
                .map(|r| r.session_id.as_str())
                .unwrap_or(session_id);
            let resolved_dialogue_key = registration
                .as_ref()
                .map(|r| r.dialogue_key.as_str())
                .unwrap_or(dialogue_key);
            let resolved_turn_id = registration
                .as_ref()
                .map(|r| r.turn_id as usize)
                .unwrap_or(turn_id);
            buf.add_dialogue_turn(
                user_id,
                user_message,
                assistant_message,
                resolved_session_id,
                resolved_turn_id,
                resolved_dialogue_key,
                message_type,
                group_id,
                message_id,
                narrative_summary,
                source_platform,
            );
        }

        // 若会话关闭则触发保存
        if let Some(ref reg) = registration {
            if !reg.closed_session_id.is_empty() {
                let closed_user_id = if reg.closed_session_user_id.is_empty() {
                    user_id.to_string()
                } else {
                    reg.closed_session_user_id.clone()
                };
                self.schedule_conversation_save(closed_user_id, reg.closed_session_id.clone());
            }
        }

        tracing::info!(
            "[后台协调] 已登记对话轮次：用户={}，会话={}，轮次={}",
            user_id,
            registration
                .as_ref()
                .map(|r| r.session_id.as_str())
                .unwrap_or(session_id),
            registration
                .as_ref()
                .map(|r| r.turn_id)
                .unwrap_or(turn_id as i64),
        );
    }

    // ── Memory Extraction ───────────────────────────────────────

    /// 解析会话 ID：优先使用传入值，否则从会话存储获取活跃会话 ID
    pub fn resolve_session_id(
        &self,
        user_id: &str,
        dialogue_key: Option<&str>,
        message_type: &str,
        group_id: Option<&str>,
        session_id: Option<&str>,
        platform: &str,
    ) -> String {
        if let Some(sid) = session_id {
            let trimmed = sid.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if let Some(ref store) = self.conversation_store {
            store.get_active_session_id(user_id, dialogue_key, message_type, group_id, platform)
        } else {
            String::new()
        }
    }

    pub async fn maybe_extract_memories(&self, user_id: &str, session_id: &str) -> Vec<MemoryItem> {
        if !self.auto_extract_memory {
            tracing::debug!("[后台协调] 自动记忆提取未启用");
            return vec![];
        }
        if self.llm_client.is_none() || self.memory_store.is_none() {
            tracing::debug!("[后台协调] 记忆提取缺少必要组件");
            return vec![];
        }

        let extract_every = self.config.extract_every_n_turns.max(1);
        let should = {
            let buf = self.extractor_buffer.lock().unwrap();
            buf.should_extract(session_id, extract_every)
        };
        if !should {
            let (pending, interval, remaining) = {
                let buf = self.extractor_buffer.lock().unwrap();
                let p = buf.get_pending_turn_count(session_id);
                let iv = extract_every as i64;
                (p as i64, iv, std::cmp::max(iv - p as i64, 0))
            };
            tracing::info!(
                "[后台协调] 自动记忆提取暂不触发：用户={}，会话={}，当前待提取轮次={}/{}，还差={} 轮",
                user_id, session_id, pending, interval, remaining,
            );
            return vec![];
        }

        match self.do_extract_memories(user_id, session_id, false).await {
            Ok(items) => {
                if !items.is_empty() {
                    self.sync_person_facts(user_id).await;
                    self.fire_memory_changed();
                }
                items
            }
            Err(e) => {
                tracing::warn!("[后台协调] 自动记忆提取失败: {}", e);
                vec![]
            }
        }
    }

    pub fn schedule_memory_extraction(&self, user_id: String, session_id: String) {
        let weak = self.weak_self();
        let sid = session_id.clone();
        self.task_manager.create_task(
            async move {
                if let Some(coord) = weak.upgrade() {
                    let _ = coord.maybe_extract_memories(&user_id, &session_id).await;
                }
            },
            Some(format!("memory-extract-{}", sid)),
        );
    }

    pub fn force_extraction(&self, user_id: String, session_id: String) {
        if self.llm_client.is_none() || self.memory_store.is_none() {
            tracing::debug!("[后台协调] 强制提取缺少必要组件");
            return;
        }
        let weak = self.weak_self();
        let sid = session_id.clone();
        self.task_manager.create_task(
            async move {
                if let Some(coord) = weak.upgrade() {
                    match coord.do_extract_memories(&user_id, &session_id, true).await {
                        Ok(items) => {
                            if !items.is_empty() {
                                coord.sync_person_facts(&user_id).await;
                                coord.fire_memory_changed();
                            }
                        }
                        Err(e) => tracing::warn!("[后台协调] 强制提取失败: {}", e),
                    }
                }
            },
            Some(format!("memory-force-extract-{}", sid)),
        );
    }

    async fn do_extract_memories(
        &self,
        user_id: &str,
        session_id: &str,
        force: bool,
    ) -> XueliResult<Vec<MemoryItem>> {
        let llm_client = self
            .llm_client
            .as_ref()
            .ok_or_else(|| crate::prelude::XueliError::Internal("llm_client 不可用".into()))?;
        let memory_store = self
            .memory_store
            .as_ref()
            .ok_or_else(|| crate::prelude::XueliError::Internal("memory_store 不可用".into()))?;

        let pending_turns: Vec<BufferTurn> = {
            let mut buf = self.extractor_buffer.lock().unwrap();
            let turns = buf.get_pending_turns(session_id);
            if turns.is_empty() {
                return Ok(vec![]);
            }
            let max_turn_id = turns.iter().map(|t| t.turn_id).max().unwrap_or(0);
            let result: Vec<BufferTurn> = turns.into_iter().cloned().collect();
            if !force {
                buf.mark_session_extracted(session_id, max_turn_id);
            }
            result
        };

        let messages: Vec<String> = pending_turns
            .iter()
            .flat_map(|t| {
                vec![
                    format!("[用户] {}", t.user),
                    format!("[助手] {}", t.assistant),
                ]
            })
            .collect();

        if messages.is_empty() {
            return Ok(vec![]);
        }

        let conversation = messages.join("\n");
        let system_prompt = self.build_extraction_system_prompt().await;
        let user_prompt = self.build_extraction_user_prompt(&conversation).await;

        let chat_messages = vec![
            ChatMessage::text("system", &system_prompt),
            ChatMessage::text("user", &user_prompt),
        ];

        let request = ChatCompletionRequest {
            model: self.llm_model.clone(),
            messages: chat_messages,
            temperature: Some(0.3),
            max_tokens: Some(1024),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = llm_client.chat_completion(&request).await.map_err(|e| {
            crate::prelude::XueliError::Internal(format!("LLM 提取调用失败: {}", e))
        })?;

        let patch = parse_extraction_response(&response.content, user_id)?;

        let mut stored_items = Vec::new();
        for item in &patch.add {
            if let Err(e) = memory_store.store(item.clone()).await {
                tracing::warn!("[后台协调] 存储提取记忆失败: {} -> {}", item.id, e);
            } else {
                stored_items.push(item.clone());
            }
        }

        tracing::debug!(
            user_id = user_id,
            msg_count = messages.len(),
            extracted = stored_items.len(),
            "[后台协调] 记忆提取完成"
        );

        if force {
            let mut buf = self.extractor_buffer.lock().unwrap();
            buf.clear_buffer(Some(session_id));
        }

        Ok(stored_items)
    }

    // ── Flush ────────────────────────────────────────────────────

    pub fn flush_conversation_session(&self, user_id: String, session_id: String) {
        let weak = self.weak_self();
        let sid = session_id.clone();
        self.task_manager.create_task(
            async move {
                if let Some(coord) = weak.upgrade() {
                    if let Err(e) = coord.finalize_session(&user_id, &session_id, true).await {
                        tracing::error!(
                            "[后台协调] 会话收尾失败：用户={}，会话={}，错误={}",
                            user_id,
                            session_id,
                            e,
                        );
                    }
                }
            },
            Some(format!("memory-flush-{}", sid)),
        );
    }

    async fn finalize_session(
        &self,
        user_id: &str,
        session_id: &str,
        extract_pending: bool,
    ) -> XueliResult<Vec<MemoryItem>> {
        self.save_conversation_and_summary(user_id, session_id, true)
            .await;

        let mut saved_memories = Vec::new();
        if extract_pending && self.auto_extract_memory {
            match self.do_extract_memories(user_id, session_id, true).await {
                Ok(items) => {
                    if !items.is_empty() {
                        self.sync_person_facts(user_id).await;
                        self.fire_memory_changed();
                    }
                    saved_memories = items;
                }
                Err(e) => tracing::warn!("[后台协调] 收尾提取失败: {}", e),
            }
            {
                let mut buf = self.extractor_buffer.lock().unwrap();
                buf.clear_buffer(Some(session_id));
            }
        }

        Ok(saved_memories)
    }

    pub async fn flush_conversation_buffers(&self) {
        if let Some(ref store) = self.conversation_store {
            if let Ok(session_ids) = store.active_session_ids().await {
                for sid in &session_ids {
                    self.save_conversation_and_summary("", sid, true).await;
                }
            }
        }
    }

    pub async fn flush(&self) {
        self.flush_conversation_buffers().await;
        self.task_manager.flush().await;
    }

    pub async fn close(&self) {
        self.stop();

        if let Some(ref store) = self.conversation_store {
            if let Ok(closed_ids) = store.close_all_sessions().await {
                for sid in &closed_ids {
                    if let Err(e) = self.finalize_session("", sid, false).await {
                        tracing::error!("[后台协调] 关闭会话收尾失败：会话={}，错误={}", sid, e);
                    }
                }
            }
        }

        self.flush_conversation_buffers().await;
        self.task_manager.cancel_all().await;
    }

    // ── Conversation Save ────────────────────────────────────────

    async fn save_conversation_and_summary(&self, _user_id: &str, session_id: &str, _force: bool) {
        if let Some(ref store) = self.conversation_store {
            if let Ok(msgs) = store.load_session(session_id).await {
                if !msgs.is_empty() {
                    if let Err(e) = store.save_conversation(session_id, &msgs).await {
                        tracing::warn!("[后台协调] 保存对话失败: {}", e);
                    }
                    if let Some(ref summary) = self.summary_service {
                        let _ = summary.refresh_session_summary(store, session_id, "").await;
                    }
                }
            }
        }
    }

    pub fn schedule_conversation_save(&self, user_id: String, session_id: String) {
        let weak = self.weak_self();
        let sid = session_id.clone();
        self.task_manager.create_task(
            async move {
                if let Some(coord) = weak.upgrade() {
                    coord
                        .save_conversation_and_summary(&user_id, &session_id, true)
                        .await;
                    tracing::debug!("[后台协调] 对话会话已保存");
                }
            },
            Some(format!("memory-save-{}", sid)),
        );
    }

    // ── Digestion ────────────────────────────────────────────────

    async fn run_digestion_cycle(&self) -> XueliResult<()> {
        let memory_store = match &self.memory_store {
            Some(s) => s,
            None => {
                tracing::debug!("[后台协调] 缺少 memory_store，跳过消化");
                return Ok(());
            }
        };
        let important_store = match &self.important_store {
            Some(s) => s,
            None => {
                tracing::debug!("[后台协调] 缺少 important_store，跳过消化");
                return Ok(());
            }
        };

        let user_ids = memory_store.get_all_user_ids().await?;
        if user_ids.is_empty() {
            return Ok(());
        }

        tracing::debug!("[后台协调] 记忆消化开始扫描，共 {} 个用户", user_ids.len());
        let mut insight_count = 0u32;

        for user_id in &user_ids {
            match self.generate_insight(user_id, important_store).await {
                Ok(Some(insight)) => {
                    tracing::info!("[后台协调] 记忆消化发现 insight for {}", user_id);
                    self.fire_memory_changed();
                    self.fire_insight(insight);
                    insight_count += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!("[后台协调] 消化生成 insight 失败 for {}: {}", user_id, e)
                }
            }
        }

        if insight_count > 0 {
            tracing::info!(
                "[后台协调] 记忆消化本轮完成，共 {} 条 insight",
                insight_count
            );
        }

        if self.consolidation_enabled {
            for user_id in &user_ids {
                if let Err(e) = self.run_consolidation(user_id).await {
                    tracing::debug!("[后台协调] 离线巩固处理用户 {} 失败: {}", user_id, e);
                }
            }
        }

        if self.merge_enabled {
            for user_id in &user_ids {
                if let Err(e) = self.run_merge_cycle(user_id).await {
                    tracing::debug!("[后台协调] 记忆合并处理用户 {} 失败: {}", user_id, e);
                }
            }
        }

        Ok(())
    }

    async fn generate_insight(
        &self,
        user_id: &str,
        important_store: &ImportantMemoryStore,
    ) -> XueliResult<Option<String>> {
        let memory_store = match &self.memory_store {
            Some(s) => s,
            None => return Ok(None),
        };
        let llm_client = match &self.llm_client {
            Some(c) => c,
            None => return Ok(None),
        };

        let memories = memory_store.get_by_user(user_id).await?;
        if memories.len() < 3 {
            return Ok(None);
        }

        let now = chrono::Utc::now();
        let recent: Vec<&MemoryItem> = memories
            .iter()
            .filter(|m| {
                let duration = now.signed_duration_since(m.created_at);
                let elapsed_days = (duration.num_hours() as f64) / 24.0;
                elapsed_days <= 7.0
            })
            .collect();

        if recent.len() < 3 {
            return Ok(None);
        }

        let memory_lines: Vec<String> = recent
            .iter()
            .rev()
            .take(20)
            .map(|m| format!("- [{}] {}", m.created_at.format("%Y-%m-%d"), m.content))
            .collect();

        let user_prompt = format!("【近期记忆】\n{}", memory_lines.join("\n"));
        let system_prompt = self.build_insight_system_prompt().await;

        let chat_messages = vec![
            ChatMessage::text("system", &system_prompt),
            ChatMessage::text("user", &user_prompt),
        ];

        let request = ChatCompletionRequest {
            model: self.llm_model.clone(),
            messages: chat_messages,
            temperature: Some(0.3),
            max_tokens: Some(512),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = match tokio::time::timeout(
            Duration::from_secs(30),
            llm_client.chat_completion(&request),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::debug!("[后台协调] 记忆消化 LLM 失败: {}", e);
                return Ok(None);
            }
            Err(_) => {
                tracing::debug!("[后台协调] 记忆消化 LLM 超时");
                return Ok(None);
            }
        };

        let insight_text = match parse_insight_response(&response.content) {
            Some(t) => t,
            None => return Ok(None),
        };

        let meta =
            serde_json::json!({"insight_type": "digested", "insight_source": "periodic_digestion"});
        let meta_str = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

        important_store
            .add_memory(
                user_id,
                &insight_text,
                "periodic_digestion",
                2,
                Some(&meta_str),
            )
            .await
            .map_err(|e| {
                crate::prelude::XueliError::Internal(format!("存储 insight 失败: {}", e))
            })?;

        Ok(Some(insight_text))
    }

    async fn run_consolidation(&self, user_id: &str) -> XueliResult<()> {
        let memory_store = match &self.memory_store {
            Some(s) => s,
            None => return Ok(()),
        };
        if self.llm_client.is_none() {
            return Ok(());
        }

        let memories = memory_store.get_by_user(user_id).await?;
        let now = chrono::Utc::now();
        let consolidation_hours = self.consolidation_hours;

        let unconsolidated: Vec<&MemoryItem> = memories
            .iter()
            .filter(|m| {
                let duration = now.signed_duration_since(m.created_at);
                (duration.num_hours() as f64) <= consolidation_hours
            })
            .collect();

        if unconsolidated.is_empty() {
            return Ok(());
        }

        let candidates: Vec<ConsolidationInput> = unconsolidated
            .iter()
            .map(|m| {
                // 从 memory_store 加载元数据获取额外字段（异步循环中批量加载）
                ConsolidationInput {
                    id: m.id.clone(),
                    content: m.content.clone(),
                    importance: m.importance,
                    mention_count: 0,
                    recall_count: 0,
                    emotional_tone: String::new(),
                    category: String::new(),
                }
            })
            .collect();

        // 批量加载元数据以填充 mention_count / recall_count / emotional_tone / category
        let mut enriched: Vec<ConsolidationInput> = Vec::new();
        for candidate in candidates {
            let mut input = candidate;
            if let Ok(Some(meta)) = memory_store.load_metadata(user_id, &input.id).await {
                if let Some(v) = meta.get("mention_count").and_then(|v| v.as_i64()) {
                    input.mention_count = v;
                }
                if let Some(v) = meta.get("recall_count").and_then(|v| v.as_i64()) {
                    input.recall_count = v;
                }
                if let Some(v) = meta.get("emotional_tone").and_then(|v| v.as_str()) {
                    input.emotional_tone = v.to_string();
                }
                if let Some(v) = meta.get("category").and_then(|v| v.as_str()) {
                    input.category = v.to_string();
                }
            }
            enriched.push(input);
        }

        let modifiers = self.batch_llm_consolidate(&enriched).await;

        for item in &modifiers {
            let mem_id = item["id"].as_str().unwrap_or("");
            let modifier_str = item["modifier"].to_string();
            if mem_id.is_empty() {
                continue;
            }
            let _ = memory_store
                .update_metadata(mem_id, "consolidation_version", "1")
                .await;
            let _ = memory_store
                .update_metadata(mem_id, "consolidated_at", &now.to_rfc3339())
                .await;
            let _ = memory_store
                .update_metadata(mem_id, "consolidated_half_life_modifier", &modifier_str)
                .await;
        }

        self.fire_memory_changed();
        Ok(())
    }

    async fn batch_llm_consolidate(
        &self,
        candidates: &[ConsolidationInput],
    ) -> Vec<serde_json::Value> {
        let llm_client = match &self.llm_client {
            Some(c) => c,
            None => {
                return candidates
                    .iter()
                    .map(|c| serde_json::json!({"id": c.id, "modifier": 1.0}))
                    .collect()
            }
        };

        let batch_size = self.consolidation_batch_size;
        let mut results: Vec<serde_json::Value> = Vec::new();

        for batch in candidates.chunks(batch_size) {
            let lines: Vec<String> = batch
                .iter()
                .map(|c| {
                    let cnt = &c.content;
                    let truncated = if cnt.len() > 120 { &cnt[..120] } else { cnt };
                    let mut extra = Vec::new();
                    if c.mention_count > 0 {
                        extra.push(format!("mention_count={}", c.mention_count));
                    }
                    if c.recall_count > 0 {
                        extra.push(format!("recall_count={}", c.recall_count));
                    }
                    if !c.emotional_tone.is_empty() {
                        extra.push(format!("emotional_tone={}", c.emotional_tone));
                    }
                    if !c.category.is_empty() {
                        extra.push(format!("category={}", c.category));
                    }
                    let extra_str = if extra.is_empty() {
                        String::new()
                    } else {
                        format!("; {}", extra.join("; "))
                    };
                    format!(
                        "- id={}; content={}; importance={}{}",
                        c.id, truncated, c.importance, extra_str,
                    )
                })
                .collect();
            let user_prompt = lines.join("\n");
            let system_prompt = "评估以下新记忆的长期半衰期调制器。返回JSON数组：[{\"id\":\"...\", \"modifier\":1.0}]，modifier>1.0=更慢遗忘，<1.0=更快遗忘。mention_count/recall_count/emotional_tone/category 为辅助参考信息。";

            let chat_messages = vec![
                ChatMessage::text("system", system_prompt),
                ChatMessage::text("user", &user_prompt),
            ];

            let request = ChatCompletionRequest {
                model: self.llm_model.clone(),
                messages: chat_messages,
                temperature: Some(0.3),
                max_tokens: Some(512),
                stream: false,
                tools: None,
                tool_choice: None,
                extra_params: Default::default(),
            };

            let batch_results = match tokio::time::timeout(
                Duration::from_secs(30),
                llm_client.chat_completion(&request),
            )
            .await
            {
                Ok(Ok(r)) => parse_consolidation_response(&r.content),
                _ => batch
                    .iter()
                    .map(|c| serde_json::json!({"id": c.id, "modifier": 1.0}))
                    .collect(),
            };

            results.extend(batch_results);
        }

        results
    }

    async fn run_merge_cycle(&self, user_id: &str) -> XueliResult<()> {
        let memory_store = match &self.memory_store {
            Some(s) => s,
            None => return Ok(()),
        };

        let memories = memory_store.get_by_user(user_id).await?;
        if memories.len() < self.merge_min_cluster_size {
            return Ok(());
        }

        let clusters = cluster_by_topic(&memories, 0.6);

        for cluster in &clusters {
            if cluster.len() < self.merge_min_cluster_size {
                continue;
            }
            let cluster_refs: Vec<&MemoryItem> = if cluster.len() > self.merge_max_batch {
                cluster[..self.merge_max_batch].iter().collect()
            } else {
                cluster.iter().collect()
            };

            let merged_content = self.llm_merge_memories(&cluster_refs).await;
            let merged_content = match merged_content {
                Some(c) => c,
                None => continue,
            };

            let merged_item = MemoryItem {
                id: format!(
                    "merged_{}_{:04x}",
                    chrono::Utc::now().format("%Y%m%d%H%M%S"),
                    (merged_content.len() as u16)
                ),
                user_id: user_id.to_string(),
                content: merged_content.clone(),
                memory_type: MemoryType::Fact,
                importance: cluster_refs
                    .iter()
                    .map(|m| m.importance)
                    .fold(0.0, f64::max),
                created_at: chrono::Utc::now(),
                last_accessed_at: chrono::Utc::now(),
                access_count: 0,
            };

            if memory_store.store(merged_item.clone()).await.is_ok() {
                for mem in &cluster_refs {
                    let _ = memory_store
                        .update_metadata(&mem.id, "patch_status", "superseded")
                        .await;
                    let _ = memory_store
                        .update_metadata(&mem.id, "merged_into_id", &merged_item.id)
                        .await;
                }
                self.fire_memory_changed();
            }
        }

        Ok(())
    }

    async fn llm_merge_memories(&self, cluster: &[&MemoryItem]) -> Option<String> {
        let llm_client = self.llm_client.as_ref()?;
        let lines: Vec<String> = cluster.iter().map(|m| format!("- {}", m.content)).collect();
        let user_prompt = format!("合并以下相似记忆，生成一条综合摘要：\n{}", lines.join("\n"));
        let system_prompt = "你是一个记忆合并助手。将多条相似记忆合并为一条综合摘要。只输出合并后的文本，不要JSON。";

        let chat_messages = vec![
            ChatMessage::text("system", system_prompt),
            ChatMessage::text("user", &user_prompt),
        ];

        let request = ChatCompletionRequest {
            model: self.llm_model.clone(),
            messages: chat_messages,
            temperature: Some(0.3),
            max_tokens: Some(512),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = tokio::time::timeout(
            Duration::from_secs(30),
            llm_client.chat_completion(&request),
        )
        .await
        .ok()?
        .ok()?;
        let content = response.content.trim().to_string();
        if content.is_empty() {
            None
        } else {
            Some(content)
        }
    }

    // ── Sync ─────────────────────────────────────────────────────

    async fn sync_person_facts(&self, user_id: &str) {
        if let Some(ref svc) = self.person_fact_service {
            if let Err(e) = svc.sync_user_facts(user_id).await {
                tracing::warn!("[后台协调] 同步人物事实失败: {}", e);
            }
        }
    }

    // ── Prompt Building (using PromptTemplateLoader) ────────────

    /// 构建记忆提取系统提示词 — 优先从模板加载，失败则兜底
    async fn build_extraction_system_prompt(&self) -> String {
        if let Ok(template) = self
            .prompt_loader
            .get_template("zh-CN", "memory_extraction")
            .await
        {
            return template;
        }
        // 兜底
        r#"你是一个记忆提取助手。从对话中提取关于用户的有意义信息。

提取规则：
- 只提取关于用户的事实、偏好、经历或观点
- 每条记忆应该是一句简洁的陈述
- 记忆类型：fact（事实）、preference（偏好）、event（经历）、opinion（观点）、relationship（关系信息）
- 重要度 0.0-1.0：1.0 表示极其重要（如姓名、关键偏好），0.5 表示一般信息
- 如果没有值得记忆的内容，返回空列表

输出 JSON 格式：
```json
{
  "memories": [
    {
      "content": "记忆内容",
      "memory_type": "fact|preference|event|opinion|relationship",
      "importance": 0.8,
      "confidence": 0.9
    }
  ]
}
```

只输出 JSON，不要额外说明。"#
        .to_string()
    }

    /// 构建记忆提取用户提示词 — 优先从模板加载，失败则兜底
    async fn build_extraction_user_prompt(&self, conversation: &str) -> String {
        if let Ok(template) = self
            .prompt_loader
            .get_template("zh-CN", "memory_extraction_user")
            .await
        {
            let vars = std::collections::HashMap::from([("conversation", conversation)]);
            return self.prompt_loader.render(&template, &vars);
        }
        // 兜底
        format!(
            "请从以下对话中提取关于用户的值得记住的信息：\n\n```\n{}\n```\n\n请输出 JSON。",
            conversation
        )
    }

    /// 构建 insight 消化系统提示词 — 优先从模板加载，失败则兜底
    async fn build_insight_system_prompt(&self) -> String {
        if let Ok(template) = self
            .prompt_loader
            .get_template("zh-CN", "insight_digestion")
            .await
        {
            return template;
        }
        // 兜底
        r#"你是记忆分析助手。分析近期记忆，判断是否有可提炼的深层洞察。

输出 JSON 格式：
```json
{
  "has_insight": true,
  "content": "洞察内容（一句简洁的陈述）",
  "confidence": 0.8
}
```

如果近期记忆中没有值得提炼的洞察，has_insight 设为 false。
只输出 JSON，不要额外说明。"#
            .to_string()
    }
}

// ── Helpers ──────────────────────────────────────────────────────

fn parse_extraction_response(content: &str, user_id: &str) -> XueliResult<MemoryPatch> {
    let text = content.trim();
    if text.is_empty() {
        return Ok(MemoryPatch {
            add: vec![],
            update: vec![],
            remove: vec![],
        });
    }

    let json_str = extract_json_from_text(text);
    let parsed: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| format!("JSON 解析失败: {e}"))?;

    let memories = match parsed.get("memories").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            return Ok(MemoryPatch {
                add: vec![],
                update: vec![],
                remove: vec![],
            })
        }
    };

    let now = chrono::Utc::now();
    let items: Vec<MemoryItem> = memories
        .iter()
        .filter_map(|m| {
            let content = m.get("content")?.as_str()?.to_string();
            let importance = m.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.5);
            let confidence = m.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.5);
            if confidence < 0.3 {
                return None;
            }
            let memory_type = m
                .get("memory_type")
                .and_then(|v| v.as_str())
                .map(|s| match s {
                    "preference" => MemoryType::Preference,
                    "event" => MemoryType::Event,
                    "opinion" => MemoryType::Opinion,
                    "relationship" => MemoryType::Relationship,
                    _ => MemoryType::Fact,
                })
                .unwrap_or(MemoryType::Fact);

            Some(MemoryItem {
                id: format!("mem_{}_{}", user_id, uuid::Uuid::new_v4().as_simple()),
                user_id: user_id.to_string(),
                content,
                memory_type,
                importance: importance.clamp(0.0, 1.0),
                created_at: now,
                last_accessed_at: now,
                access_count: 0,
            })
        })
        .collect();

    Ok(MemoryPatch {
        add: items,
        update: vec![],
        remove: vec![],
    })
}

fn extract_json_from_text(text: &str) -> String {
    let trimmed = text.trim();
    let no_fence = trimmed
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let obj_start = no_fence.find('{');
    let arr_start = no_fence.find('[');
    match (obj_start, arr_start) {
        (Some(o), Some(a)) => {
            let start = o.min(a);
            let end_char = if start == o { '}' } else { ']' };
            let end = no_fence.rfind(end_char).unwrap_or(no_fence.len() - 1);
            no_fence[start..=end].to_string()
        }
        (Some(start), None) => {
            let end = no_fence.rfind('}').unwrap_or(no_fence.len() - 1);
            no_fence[start..=end].to_string()
        }
        (None, Some(start)) => {
            let end = no_fence.rfind(']').unwrap_or(no_fence.len() - 1);
            no_fence[start..=end].to_string()
        }
        (None, None) => no_fence.to_string(),
    }
}

fn parse_insight_response(content: &str) -> Option<String> {
    let text = extract_json_from_text(content);
    let data: serde_json::Value = serde_json::from_str(&text).ok()?;
    if !data.get("has_insight")?.as_bool()? {
        return None;
    }
    let insight_text = data.get("content")?.as_str()?.trim().to_string();
    let confidence = data
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if insight_text.is_empty() || confidence < 0.7 {
        return None;
    }
    Some(insight_text)
}

fn parse_consolidation_response(content: &str) -> Vec<serde_json::Value> {
    let text = extract_json_from_text(content);
    let data: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let arr = match data.as_array() {
        Some(a) => a,
        None => return vec![],
    };
    arr.iter()
        .filter_map(|item| {
            let obj = item.as_object()?;
            let mem_id = obj.get("id")?.as_str()?.to_string();
            let modifier = obj
                .get("modifier")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0)
                .clamp(0.1, 5.0);
            Some(serde_json::json!({"id": mem_id, "modifier": modifier}))
        })
        .collect()
}

fn cluster_by_topic(memories: &[MemoryItem], min_similarity: f64) -> Vec<Vec<MemoryItem>> {
    fn tokenize(text: &str) -> Vec<String> {
        let normalized: String = text
            .chars()
            .filter(|c| c.is_alphanumeric() || (*c >= '\u{4e00}' && *c <= '\u{9fff}'))
            .collect::<String>()
            .to_lowercase();
        let chars: Vec<char> = normalized.chars().collect();
        let mut bigrams = Vec::new();
        for w in chars.windows(2) {
            bigrams.push(w.iter().collect::<String>());
        }
        bigrams
    }

    fn bigram_similarity(a: &[String], b: &[String]) -> f64 {
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        let set_a: std::collections::HashSet<&String> = a.iter().collect();
        let set_b: std::collections::HashSet<&String> = b.iter().collect();
        let intersection = set_a.intersection(&set_b).count();
        intersection as f64 / set_a.len().min(set_b.len()) as f64
    }

    let tokenized: Vec<Vec<String>> = memories.iter().map(|m| tokenize(&m.content)).collect();
    let mut clusters: Vec<Vec<MemoryItem>> = Vec::new();
    let mut used: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for (i, mem) in memories.iter().enumerate() {
        if used.contains(&i) {
            continue;
        }
        let mut cluster = vec![mem.clone()];
        used.insert(i);
        for (j, _other) in memories.iter().enumerate() {
            if used.contains(&j) || j <= i {
                continue;
            }
            let sim = bigram_similarity(&tokenized[i], &tokenized[j]);
            if sim >= min_similarity {
                cluster.push(memories[j].clone());
                used.insert(j);
            }
        }
        if cluster.len() >= 2 {
            clusters.push(cluster);
        }
    }

    clusters
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::prompt_loader::NoopPromptTemplateLoader;

    type TestCoordinator = MemoryBackgroundCoordinator<NoopPromptTemplateLoader>;

    fn make_coordinator() -> TestCoordinator {
        let config = Arc::new(MemoryConfig::default());
        let task_mgr = Arc::new(MemoryTaskManager::new());
        MemoryBackgroundCoordinator::new(config, task_mgr, Arc::new(NoopPromptTemplateLoader))
    }

    #[test]
    fn test_extract_json_from_text_plain() {
        let input = r#"{"has_insight": true, "content": "test", "confidence": 0.9}"#;
        let result = extract_json_from_text(input);
        assert!(result.contains("has_insight"));
    }

    #[test]
    fn test_extract_json_from_text_with_fence() {
        let input = "```json\n{\"key\": \"value\"}\n```";
        let result = extract_json_from_text(input);
        assert_eq!(result, "{\"key\": \"value\"}");
    }

    #[test]
    fn test_extract_json_from_text_with_fence_no_lang() {
        let input = "```\n{\"key\": \"value\"}\n```";
        let result = extract_json_from_text(input);
        assert_eq!(result, "{\"key\": \"value\"}");
    }

    #[test]
    fn test_parse_insight_response_positive() {
        let input =
            r#"{"has_insight": true, "content": "用户对技术有深层兴趣", "confidence": 0.85}"#;
        let result = parse_insight_response(input);
        assert_eq!(result.unwrap(), "用户对技术有深层兴趣");
    }

    #[test]
    fn test_parse_insight_response_negative() {
        let input = r#"{"has_insight": false, "content": "", "confidence": 0.1}"#;
        let result = parse_insight_response(input);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_insight_response_low_confidence() {
        let input = r#"{"has_insight": true, "content": "test", "confidence": 0.5}"#;
        let result = parse_insight_response(input);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_insight_response_empty_content() {
        let input = r#"{"has_insight": true, "content": "", "confidence": 0.8}"#;
        let result = parse_insight_response(input);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_consolidation_response() {
        let input = r#"[{"id": "m1", "modifier": 1.5}, {"id": "m2", "modifier": 0.8}]"#;
        let results = parse_consolidation_response(input);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["id"], "m1");
        assert!((results[0]["modifier"].as_f64().unwrap() - 1.5).abs() < 0.01);
    }

    #[test]
    fn test_parse_consolidation_response_empty() {
        let results = parse_consolidation_response("invalid json");
        assert!(results.is_empty());
    }

    #[test]
    fn test_cluster_by_topic() {
        let now = chrono::Utc::now();
        let items = vec![
            MemoryItem {
                id: "1".into(),
                user_id: "u1".into(),
                content: "用户喜欢喝咖啡".into(),
                memory_type: MemoryType::Preference,
                importance: 0.8,
                created_at: now,
                last_accessed_at: now,
                access_count: 0,
            },
            MemoryItem {
                id: "2".into(),
                user_id: "u1".into(),
                content: "用户每天早上喝咖啡".into(),
                memory_type: MemoryType::Preference,
                importance: 0.7,
                created_at: now,
                last_accessed_at: now,
                access_count: 0,
            },
            MemoryItem {
                id: "3".into(),
                user_id: "u1".into(),
                content: "用户喜欢篮球运动".into(),
                memory_type: MemoryType::Preference,
                importance: 0.6,
                created_at: now,
                last_accessed_at: now,
                access_count: 0,
            },
        ];
        let clusters = cluster_by_topic(&items, 0.4);
        assert!(!clusters.is_empty());
        let coffee_cluster = clusters.iter().find(|c| c.iter().any(|m| m.id == "1"));
        assert!(coffee_cluster.is_some());
        assert!(coffee_cluster.unwrap().iter().any(|m| m.id == "2"));
    }

    #[test]
    fn test_cluster_by_topic_single_item_no_cluster() {
        let now = chrono::Utc::now();
        let items = vec![
            MemoryItem {
                id: "1".into(),
                user_id: "u1".into(),
                content: "用户喜欢喝咖啡".into(),
                memory_type: MemoryType::Preference,
                importance: 0.8,
                created_at: now,
                last_accessed_at: now,
                access_count: 0,
            },
            MemoryItem {
                id: "2".into(),
                user_id: "u1".into(),
                content: "用户喜欢篮球运动".into(),
                memory_type: MemoryType::Preference,
                importance: 0.6,
                created_at: now,
                last_accessed_at: now,
                access_count: 0,
            },
        ];
        let clusters = cluster_by_topic(&items, 0.6);
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_coordinator_construction() {
        let coordinator = make_coordinator();
        assert!(!coordinator.is_running());
    }

    #[test]
    fn test_coordinator_builder() {
        let coordinator = make_coordinator()
            .with_auto_extract(true)
            .with_consolidation_enabled(true)
            .with_merge_enabled(true)
            .with_merge_min_cluster_size(3)
            .with_llm_model("gpt-4o".into());
        assert_eq!(coordinator.auto_extract_memory, true);
        assert_eq!(coordinator.consolidation_enabled, true);
        assert_eq!(coordinator.merge_enabled, true);
        assert_eq!(coordinator.merge_min_cluster_size, 3);
        assert_eq!(coordinator.llm_model, "gpt-4o");
    }

    #[test]
    fn test_coordinator_callbacks() {
        let coordinator = make_coordinator();

        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f1 = flag.clone();
        coordinator.set_memory_changed_callback(move || {
            f1.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        coordinator.fire_memory_changed();
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_start_stop() {
        let coordinator = make_coordinator().into_arc();
        assert!(!coordinator.is_running());
        coordinator.start(60);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(coordinator.is_running());
        coordinator.stop();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!coordinator.is_running());
    }
}
