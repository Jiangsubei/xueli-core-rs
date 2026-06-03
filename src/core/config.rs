use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

/// xueli-core 全局配置
///
/// 对应 Python 版 `xueli/src/core/config.py`
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct XueliConfig {
    /// AI 模型配置
    pub model: ModelConfig,

    /// 回复行为配置
    pub reply: ReplyConfig,

    /// 记忆系统配置
    pub memory: MemoryConfig,

    /// Timeline Gate 配置
    pub timing_gate: TimingGateConfig,

    /// 会话配置
    pub session: SessionConfig,

    /// 表情系统配置
    pub emoji: EmojiConfig,

    /// 主动分享配置
    pub proactive_share: ProactiveShareConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelConfig {
    /// 主要模型名称
    pub primary_model: String,
    /// 轻量模型（用于 timing gate / planner）
    pub light_model: String,
    /// VLM 视觉模型
    pub vision_model: Option<String>,
    /// API Base URL
    pub api_base: String,
    /// API Key
    pub api_key: String,
    /// 默认 temperature
    pub temperature: f64,
    /// 最大输出 token 数
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReplyConfig {
    /// 群聊冷却时间（秒）
    pub group_cooldown_secs: f64,
    /// 私聊冷却时间（秒）
    pub private_cooldown_secs: f64,
    /// 单次上下文窗口最大消息数
    pub max_context_messages: usize,
    /// 最大回复长度（字符数）
    pub max_reply_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryConfig {
    /// SQLite 数据库路径
    pub db_path: String,
    /// 记忆提取最小消息数
    pub extraction_min_messages: usize,
    /// BM25 检索返回数
    pub bm25_top_k: usize,
    /// 向量检索返回数
    pub vector_top_k: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TimingGateConfig {
    /// 默认主动回复概率
    pub default_proactive_probability: f64,
    /// 被 @ 时回复概率
    pub mention_reply_probability: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionConfig {
    /// 会话超时时间（秒）
    pub session_timeout_secs: u64,
    /// 每会话最大并发消息数
    pub max_concurrent_messages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmojiConfig {
    /// 是否启用表情系统
    pub enabled: bool,
    /// 表情包数据库路径
    pub db_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProactiveShareConfig {
    /// 是否启用主动分享
    pub enabled: bool,
    /// 分享间隔（秒）
    pub interval_secs: u64,
}

impl Default for XueliConfig {
    fn default() -> Self {
        Self {
            model: ModelConfig {
                primary_model: "gpt-4o".to_string(),
                light_model: "gpt-4o-mini".to_string(),
                vision_model: Some("gpt-4o".to_string()),
                api_base: "https://api.openai.com/v1".to_string(),
                api_key: String::new(),
                temperature: 0.7,
                max_tokens: 4096,
            },
            reply: ReplyConfig {
                group_cooldown_secs: 10.0,
                private_cooldown_secs: 3.0,
                max_context_messages: 50,
                max_reply_chars: 500,
            },
            memory: MemoryConfig {
                db_path: "xueli_memory.db".to_string(),
                extraction_min_messages: 5,
                bm25_top_k: 10,
                vector_top_k: 5,
            },
            timing_gate: TimingGateConfig {
                default_proactive_probability: 0.3,
                mention_reply_probability: 0.95,
            },
            session: SessionConfig {
                session_timeout_secs: 3600,
                max_concurrent_messages: 10,
            },
            emoji: EmojiConfig {
                enabled: true,
                db_path: None,
            },
            proactive_share: ProactiveShareConfig {
                enabled: false,
                interval_secs: 3600,
            },
        }
    }
}

impl XueliConfig {
    /// 从 TOML 文件加载配置
    pub fn from_file(path: &str) -> Result<Self, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("读取配置文件失败: {}", e))?;
        toml::from_str(&content).map_err(|e| format!("解析 TOML 配置失败: {}", e))
    }
}