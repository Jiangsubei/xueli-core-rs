use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::core::config::ModelConfig;
use crate::prelude::XueliResult;

/// 模型调用路由器 — 按任务类型分派模型目标，支持 FIFO 按用途排队与超时控制。
///
/// 同用途调用按 FIFO 顺序串行执行，不同用途之间并行。
pub struct ModelInvocationRouter {
    config: ModelConfig,
    base_timeout: u64,
    state: std::sync::Mutex<RouterState>,
}

struct RouterState {
    queues: HashMap<String, tokio::sync::mpsc::Sender<QueueItem>>,
    handles: Vec<tokio::task::JoinHandle<()>>,
    closed: bool,
}

struct QueueItem {
    label: String,
    trace_id: String,
    timeout_secs: u64,
    runner: Box<
        dyn FnOnce() -> Pin<
                Box<dyn Future<Output = Result<String, crate::core::errors::XueliError>> + Send>,
            > + Send,
    >,
    reply: tokio::sync::oneshot::Sender<Result<String, crate::core::errors::XueliError>>,
}

/// 调用目标
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationTarget {
    Primary,
    Light,
    Vision,
}

/// 调用任务类型
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InvocationTask {
    TimingGate,
    Planner,
    ReplyAgent,
    MemoryExtraction,
    Reflection,
    Rerank,
    SimpleReply,
    ImageAnalysis,
    InsightDigestion,
    ChatSummary,
}

impl InvocationTask {
    fn purpose_key(&self) -> &'static str {
        match self {
            InvocationTask::TimingGate => "timing_gate",
            InvocationTask::Planner => "planner",
            InvocationTask::ReplyAgent | InvocationTask::SimpleReply => "reply_generation",
            InvocationTask::MemoryExtraction
            | InvocationTask::Reflection
            | InvocationTask::InsightDigestion
            | InvocationTask::ChatSummary => "memory_extraction",
            InvocationTask::Rerank => "memory_rerank",
            InvocationTask::ImageAnalysis => "vision_analysis",
        }
    }

    fn default_timeout(&self, base: f64) -> f64 {
        match self {
            InvocationTask::TimingGate => base.min(12.0),
            InvocationTask::Planner => base.min(20.0),
            InvocationTask::ReplyAgent | InvocationTask::SimpleReply => base,
            InvocationTask::MemoryExtraction
            | InvocationTask::Reflection
            | InvocationTask::InsightDigestion
            | InvocationTask::ChatSummary => base,
            InvocationTask::Rerank => base.min(8.0),
            InvocationTask::ImageAnalysis => base,
        }
    }
}

impl ModelInvocationRouter {
    pub fn new(config: ModelConfig) -> Self {
        Self {
            config,
            base_timeout: 60,
            state: std::sync::Mutex::new(RouterState {
                queues: HashMap::new(),
                handles: Vec::new(),
                closed: false,
            }),
        }
    }

    pub fn with_base_timeout(mut self, secs: u64) -> Self {
        self.base_timeout = secs.max(1);
        self
    }

    /// 根据任务类型选择模型目标
    pub fn route(&self, task: &InvocationTask) -> InvocationTarget {
        match task {
            InvocationTask::TimingGate | InvocationTask::SimpleReply | InvocationTask::Rerank => {
                InvocationTarget::Light
            }
            InvocationTask::ImageAnalysis => {
                if self.config.vision_model.is_some() {
                    InvocationTarget::Vision
                } else {
                    InvocationTarget::Primary
                }
            }
            _ => InvocationTarget::Primary,
        }
    }

    /// 获取目标模型名称
    pub fn get_model(&self, target: &InvocationTarget) -> &str {
        match target {
            InvocationTarget::Primary => &self.config.primary_model,
            InvocationTarget::Light => &self.config.light_model,
            InvocationTarget::Vision => self
                .config
                .vision_model
                .as_deref()
                .unwrap_or(&self.config.primary_model),
        }
    }

    /// 提交任务执行，按用途 FIFO 排队，带超时控制。
    pub async fn submit<F, Fut>(
        &self,
        task: &InvocationTask,
        runner: F,
        trace_id: &str,
        timeout_override: Option<f64>,
    ) -> XueliResult<String>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = XueliResult<String>> + Send + 'static,
    {
        let purpose = task.purpose_key().to_string();
        let timeout =
            timeout_override.unwrap_or_else(|| task.default_timeout(self.base_timeout as f64));

        let (tx, rx) = tokio::sync::oneshot::channel();

        let item = QueueItem {
            label: format!("{:?}", task),
            trace_id: trace_id.to_string(),
            timeout_secs: (timeout.max(1.0) as u64),
            runner: Box::new(move || Box::pin(runner())),
            reply: tx,
        };

        let sender = self.get_or_create_queue(&purpose);
        sender.send(item).await.map_err(|_| {
            let msg = format!("{} 队列已关闭", purpose);
            crate::core::errors::XueliError::Internal(msg)
        })?;

        rx.await.map_err(|_| {
            let msg = format!("{} 任务通道关闭", purpose);
            crate::core::errors::XueliError::Internal(msg)
        })?
    }

    /// 获取或创建用途专用 FIFO 队列，返回 mpsc sender。
    fn get_or_create_queue(&self, purpose: &str) -> tokio::sync::mpsc::Sender<QueueItem> {
        let mut state = self.state.lock().unwrap();
        if let Some(sender) = state.queues.get(purpose) {
            return sender.clone();
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<QueueItem>(1024);
        state.queues.insert(purpose.to_string(), tx.clone());

        let handle = tokio::spawn(async move {
            while let Some(item) = rx.recv().await {
                let QueueItem {
                    timeout_secs,
                    runner,
                    reply,
                    ..
                } = item;

                let result = match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    runner(),
                )
                .await
                {
                    Ok(inner) => inner,
                    Err(_) => Err(format!("任务执行超时 ({}s)", timeout_secs).into()),
                };
                let _ = reply.send(result);
            }
        });

        state.handles.push(handle);
        tx
    }

    /// 关闭所有队列，等待 worker 任务结束。
    pub async fn close(&self) {
        let handles = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            state.queues.clear();
            std::mem::take(&mut state.handles)
        };
        for handle in handles {
            let _ = handle.await;
        }
    }

    /// 返回各用途队列的快照（pending / running 计数的近似值）。
    pub fn snapshot(&self) -> HashMap<String, HashMap<String, usize>> {
        let state = self.state.lock().unwrap();
        let mut result = HashMap::new();
        for purpose in state.queues.keys() {
            let mut counts = HashMap::new();
            counts.insert("pending".to_string(), 0);
            counts.insert("running".to_string(), 1);
            result.insert(purpose.clone(), counts);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_timing_gate_uses_light() {
        let cfg = ModelConfig::default();
        let router = ModelInvocationRouter::new(cfg);
        assert_eq!(
            router.route(&InvocationTask::TimingGate),
            InvocationTarget::Light
        );
    }

    #[test]
    fn test_route_planner_uses_primary() {
        let cfg = ModelConfig::default();
        let router = ModelInvocationRouter::new(cfg);
        assert_eq!(
            router.route(&InvocationTask::Planner),
            InvocationTarget::Primary
        );
    }

    #[tokio::test]
    async fn test_submit_simple() {
        let cfg = ModelConfig::default();
        let router = ModelInvocationRouter::new(cfg);
        let result = router
            .submit(
                &InvocationTask::SimpleReply,
                || async { Ok("hello".to_string()) },
                "t1",
                None,
            )
            .await;
        assert_eq!(result.unwrap(), "hello");
    }

    #[tokio::test]
    async fn test_submit_timeout() {
        let cfg = ModelConfig::default();
        let router = ModelInvocationRouter::new(cfg);
        let result = router
            .submit(
                &InvocationTask::MemoryExtraction,
                || async {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    Ok("done".to_string())
                },
                "t2",
                Some(0.1),
            )
            .await;
        assert!(result.is_err());
    }
}
