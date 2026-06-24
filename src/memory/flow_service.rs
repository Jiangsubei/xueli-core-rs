use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::character::card_service::CharacterCardService;
use crate::character::narrative::NarrativeService;
use crate::core::config::MemoryDisputeConfig;
use crate::core::types::{MemoryItem, MemoryPatch};
use crate::memory::extraction::extractor::MemoryExtractor;
use crate::memory::extraction::reflection::MemoryReflection;
use crate::memory::internal::background::MemoryBackgroundCoordinator;
use crate::memory::manager::MemoryManager;
use crate::memory::memory_dispute_resolver::MemoryDisputeResolver;
use crate::memory::stores::fact_evidence::{FactEvidence, SqliteFactEvidenceStore};
use crate::prelude::XueliResult;
use crate::traits::ai_client::AIClient;
use crate::traits::prompt_template::PromptTemplateLoader;

const MAX_QUEUE_SIZE: usize = 256;

/// 记忆流服务 — 异步队列处理记忆提取与持久化
///
/// 对应 Python 版 `src.memory.memory_flow_service.MemoryFlowService`
pub struct MemoryFlowService<L: PromptTemplateLoader + 'static> {
    pub tx: mpsc::Sender<MemoryJob>,
    rx: Option<mpsc::Receiver<MemoryJob>>,
    pub memory_manager: Arc<MemoryManager<L>>,
    dispute_resolver: MemoryDisputeResolver,
    evidence_store: Option<Arc<SqliteFactEvidenceStore>>,
    recent_reply_keys: std::sync::Mutex<HashMap<String, u64>>,
    running: Arc<AtomicBool>,
    handle: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,

    // Phase 1.1: LLM 服务注入
    extractor: Option<Arc<MemoryExtractor<dyn AIClient, L>>>,
    background_coordinator: Option<Arc<MemoryBackgroundCoordinator<L>>>,
    memory_reflection: Option<Arc<MemoryReflection<dyn AIClient, L>>>,

    // Phase 1.2: 角色成长/亲密度/叙事
    character_card_service: Option<Arc<CharacterCardService>>,
    narrative_service: Option<Arc<NarrativeService>>,
}

/// 记忆作业
#[derive(Debug, Clone)]
pub enum MemoryJob {
    /// 从对话提取记忆
    ExtractFromConversation {
        conversation_id: String,
        messages: Vec<String>,
    },
    /// 消化离线记忆
    DigestInsight { user_id: String },
    /// 触发反思
    Reflect { user_id: String },
    /// 应用 Patch
    ApplyPatch(MemoryPatch),
    /// 注册对话轮次
    RegisterDialogue {
        user_id: String,
        user_message: String,
        assistant_message: String,
        dialogue_key: String,
        scope_type: String,
        group_id: String,
        message_id: String,
        image_description: String,
        narrative_summary: String,
        platform: String,
        warmth_guidance: String,
        user_emotion_label: String,
        intimacy_delta: f64,
    },
}

impl<L: PromptTemplateLoader + 'static> MemoryFlowService<L> {
    pub fn new(
        memory_manager: Arc<MemoryManager<L>>,
        dispute_config: Option<MemoryDisputeConfig>,
        evidence_store: Option<Arc<SqliteFactEvidenceStore>>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(MAX_QUEUE_SIZE);
        let config = dispute_config.unwrap_or_default();
        Self {
            tx,
            rx: Some(rx),
            memory_manager,
            dispute_resolver: MemoryDisputeResolver::new(config),
            evidence_store,
            recent_reply_keys: std::sync::Mutex::new(HashMap::new()),
            running: Arc::new(AtomicBool::new(false)),
            handle: tokio::sync::Mutex::new(None),
            extractor: None,
            background_coordinator: None,
            memory_reflection: None,
            character_card_service: None,
            narrative_service: None,
        }
    }

    /// 设置记忆提取器
    pub fn with_extractor(mut self, extractor: Arc<MemoryExtractor<dyn AIClient, L>>) -> Self {
        self.extractor = Some(extractor);
        self
    }

    /// 设置后台协调器
    pub fn with_background_coordinator(
        mut self,
        coordinator: Arc<MemoryBackgroundCoordinator<L>>,
    ) -> Self {
        self.background_coordinator = Some(coordinator);
        self
    }

    /// 设置记忆反思
    pub fn with_memory_reflection(
        mut self,
        reflection: Arc<MemoryReflection<dyn AIClient, L>>,
    ) -> Self {
        self.memory_reflection = Some(reflection);
        self
    }

    /// 设置角色卡服务
    pub fn with_character_card_service(mut self, service: Arc<CharacterCardService>) -> Self {
        self.character_card_service = Some(service);
        self
    }

    /// 设置叙事服务
    pub fn with_narrative_service(mut self, service: Arc<NarrativeService>) -> Self {
        self.narrative_service = Some(service);
        self
    }

    /// 启动后台处理循环
    pub async fn start(&self) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        // start() 使用外部 run() 模式：调用者自行管理 rx 循环
        // 此处仅标记 running 状态
        info!("[MemoryFlow] 已标记为运行状态");
    }

    /// 关闭记忆流服务
    pub async fn close(&self) {
        self.running.store(false, Ordering::SeqCst);
        let mut guard = self.handle.lock().await;
        if let Some(handle) = guard.take() {
            handle.abort();
        }
        info!("[MemoryFlow] 已关闭");
    }

    /// 是否正在运行
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 提交记忆作业（有界队列，满时丢弃并记录警告）
    pub fn submit(&self, job: MemoryJob) -> XueliResult<()> {
        match self.tx.try_send(job) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!("[MemoryFlow] 记忆队列满，丢弃任务");
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                Err(format!("记忆作业提交失败: channel 已关闭").into())
            }
        }
    }

    /// 回复生成后的副作用入口
    pub fn on_reply_generated(
        &self,
        user_id: &str,
        user_message: &str,
        assistant_message: &str,
        dialogue_key: &str,
        scope_type: &str,
        group_id: &str,
        message_id: &str,
        image_description: &str,
        narrative_summary: &str,
        platform: &str,
        warmth_guidance: &str,
        user_emotion_label: &str,
        intimacy_delta: f64,
    ) {
        let has_text = !user_message.trim().is_empty();
        let has_image = !image_description.trim().is_empty();
        if !has_text && !has_image {
            return;
        }

        let dedupe_key =
            Self::build_dedupe_key(dialogue_key, message_id, user_message, assistant_message);
        if self.check_dedupe_timestamp(&dedupe_key) {
            return;
        }

        let job = MemoryJob::RegisterDialogue {
            user_id: user_id.to_string(),
            user_message: user_message.to_string(),
            assistant_message: assistant_message.to_string(),
            dialogue_key: dialogue_key.to_string(),
            scope_type: scope_type.to_string(),
            group_id: group_id.to_string(),
            message_id: message_id.to_string(),
            image_description: image_description.to_string(),
            narrative_summary: narrative_summary.to_string(),
            platform: platform.to_string(),
            warmth_guidance: warmth_guidance.to_string(),
            user_emotion_label: user_emotion_label.to_string(),
            intimacy_delta,
        };
        let _ = self.tx.try_send(job);
    }

    fn check_dedupe_timestamp(&self, dedupe_key: &str) -> bool {
        if dedupe_key.is_empty() {
            return false;
        }
        let now = chrono::Utc::now().timestamp() as u64;
        let expire_before = now.saturating_sub(30);
        let mut keys = self.recent_reply_keys.lock().unwrap();
        keys.retain(|_, ts| *ts >= expire_before);
        if keys.contains_key(dedupe_key) {
            return true;
        }
        keys.insert(dedupe_key.to_string(), now);
        false
    }

    /// 检查是否需要去重（基于事件和回复内容）
    pub fn should_dedupe(&self, dedupe_key: &str) -> bool {
        self.check_dedupe_timestamp(dedupe_key)
    }

    /// 构建去重键
    pub fn build_dedupe_key(
        session_key: &str,
        message_id: &str,
        user_message: &str,
        reply_text: &str,
    ) -> String {
        let payload = format!(
            "{}|{}|{}|{}",
            session_key, message_id, user_message, reply_text
        );
        if payload.trim().is_empty() {
            return String::new();
        }
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(payload.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// 启动后台处理循环（实例方法版本，支持 LLM 服务注入）
    pub async fn run(&mut self) {
        let mut rx = self.rx.take().expect("MemoryFlowService::run 只能调用一次");
        while let Some(job) = rx.recv().await {
            match job {
                MemoryJob::ApplyPatch(patch) => {
                    let add_count = patch.add.len();
                    let update_count = patch.update.len();
                    let remove_count = patch.remove.len();
                    match self.memory_manager.apply_patch(patch).await {
                        Ok(()) => {
                            debug!(
                                add = add_count,
                                update = update_count,
                                remove = remove_count,
                                "[MemoryFlow] Patch 应用成功"
                            );
                        }
                        Err(e) => {
                            info!("[MemoryFlow] Patch 应用失败: {e}");
                        }
                    }
                }
                MemoryJob::ExtractFromConversation {
                    conversation_id,
                    messages,
                } => {
                    if let Some(ref extractor) = self.extractor {
                        match extractor.extract(&conversation_id, &messages).await {
                            Ok(patch) => {
                                let _ = self.memory_manager.apply_patch(patch).await;
                            }
                            Err(e) => {
                                debug!(
                                    conv_id = %conversation_id,
                                    "[MemoryFlow] 对话记忆提取失败: {e}"
                                );
                            }
                        }
                    } else {
                        debug!(
                            conv_id = %conversation_id,
                            msg_count = messages.len(),
                            "[MemoryFlow] 对话记忆提取（extractor 未配置，跳过）"
                        );
                    }
                }
                MemoryJob::DigestInsight { user_id } => {
                    if let Some(ref coordinator) = self.background_coordinator {
                        match coordinator.digest_user(&user_id).await {
                            Ok(Some(insight)) => {
                                debug!(
                                    user_id = %user_id,
                                    insight = %insight,
                                    "[MemoryFlow] 离线消化生成 insight"
                                );
                            }
                            Ok(None) => {
                                debug!(
                                    user_id = %user_id,
                                    "[MemoryFlow] 离线消化未生成 insight"
                                );
                            }
                            Err(e) => {
                                debug!(
                                    user_id = %user_id,
                                    "[MemoryFlow] 离线消化失败: {e}"
                                );
                            }
                        }
                    } else {
                        debug!(
                            user_id = %user_id,
                            "[MemoryFlow] 离线消化（background_coordinator 未配置，跳过）"
                        );
                    }
                }
                MemoryJob::Reflect { user_id } => {
                    if let Some(ref reflection) = self.memory_reflection {
                        let existing = self
                            .memory_manager
                            .get_by_user(&user_id)
                            .await
                            .unwrap_or_default();
                        if existing.is_empty() {
                            debug!(
                                user_id = %user_id,
                                "[MemoryFlow] 记忆反思（无已有记忆，跳过）"
                            );
                        } else {
                            let empty = vec![];
                            match reflection.reflect(&existing, &empty).await {
                                Ok(result) => {
                                    if result.has_conflict {
                                        debug!(
                                            user_id = %user_id,
                                            conflict_count = result.conflicts.len(),
                                            "[MemoryFlow] 记忆反思发现冲突"
                                        );
                                    }
                                }
                                Err(e) => {
                                    debug!(
                                        user_id = %user_id,
                                        "[MemoryFlow] 记忆反思失败: {e}"
                                    );
                                }
                            }
                        }
                    } else {
                        debug!(
                            user_id = %user_id,
                            "[MemoryFlow] 记忆反思（memory_reflection 未配置，跳过）"
                        );
                    }
                }
                MemoryJob::RegisterDialogue {
                    user_id,
                    user_message,
                    assistant_message,
                    dialogue_key,
                    scope_type,
                    group_id,
                    message_id,
                    image_description,
                    narrative_summary,
                    platform,
                    warmth_guidance,
                    user_emotion_label,
                    intimacy_delta,
                } => {
                    let _ = self
                        .memory_manager
                        .register_dialogue_turn(
                            &user_id,
                            &user_message,
                            &assistant_message,
                            &dialogue_key,
                            &scope_type,
                            &group_id,
                            &message_id,
                            &image_description,
                            &narrative_summary,
                            &platform,
                        )
                        .await;

                    // Phase 1.2: 角色成长/亲密度/叙事
                    if let Some(ref card_service) = self.character_card_service {
                        // 记录温暖引导信号
                        if !warmth_guidance.is_empty() {
                            let _ =
                                card_service.record_interaction_signal(&user_id, &warmth_guidance);
                        }
                        // 记录用户情绪标签
                        if !user_emotion_label.is_empty() {
                            let _ = card_service
                                .record_interaction_signal(&user_id, &user_emotion_label);
                        }
                        // 更新亲密度
                        if intimacy_delta != 0.0 {
                            card_service.update_intimacy(&user_id, intimacy_delta, false);
                        }
                    }

                    // 叙事事件记录
                    if let Some(ref narrative) = self.narrative_service {
                        let event_desc = format!(
                            "用户: {}, 回复: {}",
                            &user_message.chars().take(100).collect::<String>(),
                            &assistant_message.chars().take(100).collect::<String>(),
                        );
                        narrative.add_event(&user_id, &event_desc, 0.5);
                    }

                    debug!(
                        user_id = %user_id,
                        dialogue_key = %dialogue_key,
                        "[MemoryFlow] 注册对话轮次"
                    );
                }
            }
        }
        info!("[MemoryFlow] 处理循环已退出");
    }

    /// 处理记忆争议（从记忆元数据中提取争议并记录证据）
    pub async fn process_memory_disputes(
        &self,
        user_id: &str,
        memories: &[(String, MemoryItem)],
    ) -> XueliResult<()> {
        let evidence_store = match &self.evidence_store {
            Some(store) => store,
            None => return Ok(()),
        };

        let existing_records = evidence_store.get_by_user(user_id).await?;
        let existing_ids: std::collections::HashSet<String> = existing_records
            .iter()
            .map(|r| r.source_memory_id.clone())
            .collect();

        for (memory_type, item) in memories {
            let memory_id = &item.id;
            if memory_id.is_empty() || existing_ids.contains(memory_id) {
                continue;
            }

            let metadata = serde_json::json!({"memory_type": memory_type});
            let decision = self
                .dispute_resolver
                .resolve_from_memory_metadata(&metadata);

            if decision.level == "ignore" {
                continue;
            }

            let record = FactEvidence {
                id: format!("ev_{}_{}", user_id, memory_id),
                fact_id: memory_id.clone(),
                source_memory_id: memory_id.clone(),
                conversation_id: String::new(),
                message_id: String::new(),
                evidence_text: decision.summary.clone(),
                created_at: chrono::Utc::now(),
            };

            if let Err(e) = evidence_store.store(record).await {
                warn!("[MemoryFlow] 证据记录失败: {e}");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{MemoryItem, MemoryType};
    use crate::memory::stores::memory_item::SqliteMemoryItemStore;
    use chrono::Utc;

    fn make_item(id: &str, content: &str) -> MemoryItem {
        MemoryItem {
            id: id.to_string(),
            user_id: "u1".to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Fact,
            importance: 0.5,
            created_at: Utc::now(),
            last_accessed_at: Utc::now(),
            access_count: 0,
        }
    }

    #[tokio::test]
    async fn test_flow_apply_patch() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());
        let config = crate::core::config::MemoryConfig::default();
        let mgr = Arc::new(
            MemoryManager::new(
                Arc::new(config),
                store,
                Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            )
            .unwrap(),
        );
        let mut service = MemoryFlowService::new(mgr.clone(), None, None);

        let patch = MemoryPatch {
            add: vec![make_item("f1", "内容1"), make_item("f2", "内容2")],
            update: vec![],
            remove: vec![],
        };
        service.submit(MemoryJob::ApplyPatch(patch)).unwrap();

        // 运行一轮处理
        tokio::select! {
            _ = service.run() => {},
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
        }

        let items = mgr.get_by_user("u1").await.unwrap();
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn test_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());
        let config = crate::core::config::MemoryConfig::default();
        let mgr = Arc::new(
            MemoryManager::new(
                Arc::new(config),
                store,
                Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            )
            .unwrap(),
        );
        let service: MemoryFlowService<crate::services::prompt_loader::NoopPromptTemplateLoader> =
            MemoryFlowService::new(mgr, None, None);

        let key = "test_key";
        assert!(!service.should_dedupe(key));
        assert!(service.should_dedupe(key));
    }

    #[test]
    fn test_build_dedupe_key() {
        let key1 = MemoryFlowService::<crate::services::prompt_loader::NoopPromptTemplateLoader>::build_dedupe_key("s1", "m1", "hello", "hi");
        let key2 = MemoryFlowService::<crate::services::prompt_loader::NoopPromptTemplateLoader>::build_dedupe_key("s1", "m1", "hello", "hi");
        let key3 = MemoryFlowService::<crate::services::prompt_loader::NoopPromptTemplateLoader>::build_dedupe_key("s1", "m1", "hello", "hey");
        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
        assert!(!key1.is_empty());
    }
}
