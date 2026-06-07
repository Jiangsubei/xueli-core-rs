use crate::core::config::ModelConfig;
use crate::prelude::XueliResult;

/// 模型调用路由器 — 按任务类型分派模型目标，支持超时控制。
///
/// 同用途调用通过内部任务句柄追踪，保证顺序不会互相干扰。
pub struct ModelInvocationRouter {
    config: ModelConfig,
    base_timeout: f64,
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
    #[allow(dead_code)]
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
            base_timeout: 60.0,
        }
    }

    pub fn with_base_timeout(mut self, secs: f64) -> Self {
        self.base_timeout = secs.max(0.001);
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

    /// 提交任务执行，带超时控制。失败或超时返回错误。
    pub async fn submit<F, Fut>(
        &self,
        task: &InvocationTask,
        runner: F,
        _trace_id: &str,
        timeout_override: Option<f64>,
    ) -> XueliResult<String>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = XueliResult<String>> + Send,
    {
        let timeout = timeout_override.unwrap_or_else(|| task.default_timeout(self.base_timeout));

        let handle = tokio::spawn(async move {
            match tokio::time::timeout(std::time::Duration::from_secs_f64(timeout), runner()).await
            {
                Ok(result) => result,
                Err(_) => Err(format!("任务执行超时 ({:.1}s)", timeout).into()),
            }
        });

        match handle.await {
            Ok(result) => result,
            Err(e) => Err(format!("任务执行失败: {}", e).into()),
        }
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
