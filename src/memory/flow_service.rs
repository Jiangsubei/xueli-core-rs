use tokio::sync::mpsc;

use crate::core::types::MemoryPatch;

/// 记忆流服务 — 异步队列处理记忆提取与持久化
pub struct MemoryFlowService {
    tx: mpsc::UnboundedSender<MemoryJob>,
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
    DigestInsight {
        user_id: String,
    },
    /// 触发反思
    Reflect {
        user_id: String,
    },
    /// 应用 Patch
    ApplyPatch(MemoryPatch),
}

impl MemoryFlowService {
    pub fn new(buffer_size: usize) -> (Self, mpsc::UnboundedReceiver<MemoryJob>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// 提交记忆作业
    pub fn submit(&self, job: MemoryJob) -> Result<(), String> {
        self.tx
            .send(job)
            .map_err(|e| format!("记忆作业提交失败: {}", e))
    }

    /// 启动后台处理循环
    pub async fn run(
        rx: &mut mpsc::UnboundedReceiver<MemoryJob>,
    ) -> Result<(), String> {
        while let Some(job) = rx.recv().await {
            match job {
                MemoryJob::ExtractFromConversation {
                    conversation_id,
                    messages,
                } => {
                    let _ = (conversation_id, messages);
                    // TODO: 调用 MemoryExtractor
                }
                MemoryJob::DigestInsight { user_id: _ } => {
                    // TODO: 调用 InsightDigestion
                }
                MemoryJob::Reflect { user_id: _ } => {
                    // TODO: 调用 Reflection
                }
                MemoryJob::ApplyPatch(_patch) => {
                    // TODO: 应用 Patch
                }
            }
        }
        Ok(())
    }
}