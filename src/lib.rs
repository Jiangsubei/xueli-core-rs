pub mod adapters;
pub mod character;
pub mod core;
pub mod emoji;
pub mod handlers;
pub mod memory;
pub mod plugins;
pub mod proactive_share;
pub mod services;
pub mod signals;
/// xueli-core — 可复用 AI 对话框架 Rust 标准库
///
/// 提供完整的对话管线：TimingGate → Planner → ContextBuilder → ReplyAgent → MemoryFlow
/// 所有可替换组件通过 trait 抽象，支持下游定制。
pub mod traits;
pub mod util;

// 预导入 — 常用类型和 trait
pub mod prelude {
    pub use crate::core::errors::*;
    pub use crate::core::scope::*;
    pub use crate::core::types::*;
    pub use crate::traits::ai_client::AIClient;
    pub use crate::traits::platform_adapter::PlatformAdapter;
    pub use crate::traits::prompt_template::PromptTemplateLoader;
    pub use crate::traits::timing_gate::TimingGateStrategy;
    pub use crate::traits::tokenizer::Tokenizer;
    pub use crate::traits::tool_calling::ToolCallingStrategy;
}
