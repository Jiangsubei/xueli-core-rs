use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::core::config::MemoryDisputeConfig;
use crate::core::types::{MemoryItem, MemoryPatch};
use crate::memory::manager::MemoryManager;
use crate::memory::memory_dispute_resolver::MemoryDisputeResolver;
use crate::memory::stores::fact_evidence::{FactEvidence, SqliteFactEvidenceStore};
use crate::prelude::XueliResult;

/// 记忆流服务 — 异步队列处理记忆提取与持久化
///
/// 对应 Python 版 `src.memory.memory_flow_service.MemoryFlowService`
pub struct MemoryFlowService {
    tx: mpsc::UnboundedSender<MemoryJob>,
    #[allow(dead_code)]
    memory_manager: Arc<MemoryManager>,
    dispute_resolver: MemoryDisputeResolver,
    evidence_store: Option<Arc<SqliteFactEvidenceStore>>,
    recent_reply_keys: std::sync::Mutex<HashMap<String, f64>>,
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
    },
}

impl MemoryFlowService {
    pub fn new(
        memory_manager: Arc<MemoryManager>,
        dispute_config: Option<MemoryDisputeConfig>,
        evidence_store: Option<Arc<SqliteFactEvidenceStore>>,
    ) -> (Self, mpsc::UnboundedReceiver<MemoryJob>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let config = dispute_config.unwrap_or_default();
        (
            Self {
                tx,
                memory_manager,
                dispute_resolver: MemoryDisputeResolver::new(config),
                evidence_store,
                recent_reply_keys: std::sync::Mutex::new(HashMap::new()),
            },
            rx,
        )
    }

    /// 提交记忆作业
    pub fn submit(&self, job: MemoryJob) -> XueliResult<()> {
        self.tx
            .send(job)
            .map_err(|e| format!("记忆作业提交失败: {}", e).into())
    }

    /// 检查是否需要去重（基于事件和回复内容）
    pub fn should_dedupe(&self, dedupe_key: &str) -> bool {
        if dedupe_key.is_empty() {
            return false;
        }
        let now = chrono::Utc::now().timestamp() as f64;
        let expire_before = now - 30.0;

        let mut keys = self.recent_reply_keys.lock().unwrap();
        keys.retain(|_, ts| *ts >= expire_before);

        if keys.contains_key(dedupe_key) {
            return true;
        }
        keys.insert(dedupe_key.to_string(), now);
        false
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
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        payload.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// 启动后台处理循环
    pub async fn run(
        memory_manager: Arc<MemoryManager>,
        _dispute_resolver: MemoryDisputeResolver,
        _evidence_store: Option<Arc<SqliteFactEvidenceStore>>,
        rx: &mut mpsc::UnboundedReceiver<MemoryJob>,
    ) {
        while let Some(job) = rx.recv().await {
            match job {
                MemoryJob::ApplyPatch(patch) => {
                    let add_count = patch.add.len();
                    let update_count = patch.update.len();
                    let remove_count = patch.remove.len();
                    match memory_manager.apply_patch(patch).await {
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
                    debug!(
                        conv_id = %conversation_id,
                        msg_count = messages.len(),
                        "[MemoryFlow] 对话记忆提取（待接入 LLM）"
                    );
                }
                MemoryJob::DigestInsight { user_id } => {
                    debug!(
                        user_id = %user_id,
                        "[MemoryFlow] 离线消化（待接入 LLM）"
                    );
                }
                MemoryJob::Reflect { user_id } => {
                    debug!(
                        user_id = %user_id,
                        "[MemoryFlow] 记忆反思（待接入 LLM）"
                    );
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
                } => {
                    debug!(
                        user_id = %user_id,
                        dialogue_key = %dialogue_key,
                        "[MemoryFlow] 注册对话轮次"
                    );
                    // TODO: 接入 conversation_store 注册对话轮次
                    let _ = (
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
    use crate::core::config::MemoryConfig;
    use crate::core::types::{MemoryItem, MemoryType};
    use crate::memory::stores::memory_item::SqliteMemoryItemStore;
    use chrono::Utc;

    fn test_config() -> MemoryConfig {
        MemoryConfig {
            enabled: true,
            db_path: ":memory:".to_string(),
            storage_backend: "sqlite".to_string(),
            extraction_min_messages: 5,
            bm25_top_k: 10,
            vector_top_k: 5,
            rerank_top_k: 20,
            dynamic_memory_limit: 8,
            dispute: Default::default(),
            auto_extract: true,
            extract_every_n_turns: 3,
            decay: Default::default(),
            retrieval_weights: Default::default(),
        }
    }

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
        let mgr = Arc::new(MemoryManager::new(Arc::new(test_config()), store).unwrap());
        let (service, mut rx) = MemoryFlowService::new(mgr.clone(), None, None);

        let patch = MemoryPatch {
            add: vec![make_item("f1", "内容1"), make_item("f2", "内容2")],
            update: vec![],
            remove: vec![],
        };
        service.submit(MemoryJob::ApplyPatch(patch)).unwrap();

        // 运行一轮处理
        tokio::select! {
            _ = MemoryFlowService::run(mgr.clone(), MemoryDisputeResolver::new(Default::default()), None, &mut rx) => {},
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
        }

        let items = mgr.get_by_user("u1").await.unwrap();
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn test_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryItemStore::new(dir.path()).unwrap());
        let mgr = Arc::new(MemoryManager::new(Arc::new(test_config()), store).unwrap());
        let (service, _rx) = MemoryFlowService::new(mgr, None, None);

        let key = "test_key";
        assert!(!service.should_dedupe(key));
        assert!(service.should_dedupe(key));
    }

    #[test]
    fn test_build_dedupe_key() {
        let key1 = MemoryFlowService::build_dedupe_key("s1", "m1", "hello", "hi");
        let key2 = MemoryFlowService::build_dedupe_key("s1", "m1", "hello", "hi");
        let key3 = MemoryFlowService::build_dedupe_key("s1", "m1", "hello", "hey");
        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
        assert!(!key1.is_empty());
    }
}
