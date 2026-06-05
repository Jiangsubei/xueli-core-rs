use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::core::types::MemoryPatch;
use crate::memory::manager::MemoryManager;
use crate::prelude::XueliResult;

/// 记忆流服务 — 异步队列处理记忆提取与持久化
pub struct MemoryFlowService {
    tx: mpsc::UnboundedSender<MemoryJob>,
    memory_manager: Arc<MemoryManager>,
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
}

impl MemoryFlowService {
    pub fn new(
        _buffer_size: usize,
        memory_manager: Arc<MemoryManager>,
    ) -> (Self, mpsc::UnboundedReceiver<MemoryJob>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, memory_manager }, rx)
    }

    /// 提交记忆作业
    pub fn submit(&self, job: MemoryJob) -> XueliResult<()> {
        self.tx
            .send(job)
            .map_err(|e| format!("记忆作业提交失败: {}", e).into())
    }

    /// 启动后台处理循环
    pub async fn run(
        memory_manager: Arc<MemoryManager>,
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
            }
        }
        info!("[MemoryFlow] 处理循环已退出");
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
            db_path: ":memory:".to_string(),
            extraction_min_messages: 5,
            bm25_top_k: 10,
            vector_top_k: 5,
            dispute: Default::default(),
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
        let mgr = Arc::new(MemoryManager::new(Arc::new(test_config()), store));
        let (service, mut rx) = MemoryFlowService::new(10, mgr.clone());

        let patch = MemoryPatch {
            add: vec![make_item("f1", "内容1"), make_item("f2", "内容2")],
            update: vec![],
            remove: vec![],
        };
        service.submit(MemoryJob::ApplyPatch(patch)).unwrap();

        // 运行一轮处理
        tokio::select! {
            _ = MemoryFlowService::run(mgr.clone(), &mut rx) => {},
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
        }

        let items = mgr.get_by_user("u1").await.unwrap();
        assert_eq!(items.len(), 2);
    }
}
