use crate::core::config::ModelConfig;

/// 模型调用路由器 — 根据任务复杂度分派不同模型
pub struct ModelInvocationRouter {
    config: ModelConfig,
}

/// 调用目标
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationTarget {
    Primary,
    Light,
    Vision,
}

impl ModelInvocationRouter {
    pub fn new(config: ModelConfig) -> Self {
        Self { config }
    }

    /// 根据任务类型选择模型
    pub fn route(&self, task: &InvocationTask) -> InvocationTarget {
        match task {
            InvocationTask::TimingGate
            | InvocationTask::SimpleReply
            | InvocationTask::Rerank => InvocationTarget::Light,
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
            InvocationTarget::Vision => {
                self.config.vision_model.as_deref().unwrap_or(&self.config.primary_model)
            }
        }
    }
}

/// 调用任务类型
#[derive(Debug, Clone, PartialEq, Eq)]
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