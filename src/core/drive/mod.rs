//! 内驱力系统 — 三层状态模型、事件规则、LLM 反思、独立调度器。
//!
//! 三层结构：
//!   情绪层 (Affective): PAD 向量 — valence/arousal/dominance
//!   动机层 (Motivational): baseline + 瞬时_offset 双层结构
//!   关系层 (Relational): intimacy/trust/attention_weight，按作用域隔离

pub mod engine;
pub mod event_rules;
pub mod models;
pub mod reflection;
pub mod scheduler;
pub mod store;

pub use engine::DriveEngine;
pub use event_rules::{build_default_rule_set, DriveEventRuleEngine, WeightAdjustment};
pub use models::{
    AffectiveState, DriveContext, DriveSnapshot, EventLogEntry, EventRule, EventRuleSet,
    MotivationalDimension, MotivationalKey, PADVector, ReflectionOutput, RelationalState,
    RuleWeightAdjustment,
};
pub use reflection::{DriveReflection, DynTemplateLoader};
pub use scheduler::DriveScheduler;
pub use store::DriveStore;
