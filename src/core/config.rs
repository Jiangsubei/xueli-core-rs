use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prelude::XueliResult;

/// xueli-core 全局配置
///
/// 对应 Python 版 `xueli/src/core/config.py`
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct XueliConfig {
    /// AI 模型配置
    pub model: ModelConfig,

    /// 视觉服务配置
    #[serde(default)]
    pub vision: VisionServiceConfig,

    /// 回复行为配置
    pub reply: ReplyConfig,

    /// 机器人行为配置
    #[serde(default)]
    pub bot_behavior: BotBehaviorConfig,

    /// 记忆系统配置
    pub memory: MemoryConfig,

    /// Timeline Gate 配置
    pub timing_gate: TimingGateConfig,

    /// 规划窗口配置
    #[serde(default)]
    pub planning_window: PlanningWindowConfig,

    /// 会话配置
    pub session: SessionConfig,

    /// 表情系统配置
    pub emoji: EmojiConfig,

    /// 主动分享配置
    #[serde(default)]
    pub proactive_share: ProactiveShareConfig,

    /// 身份配置
    pub identity: IdentityConfig,

    /// 角色成长配置
    #[serde(default)]
    pub character_growth: CharacterGrowthConfig,

    /// 记忆冲突裁决配置
    #[serde(default)]
    pub memory_dispute: MemoryDisputeConfig,

    /// 群聊回复决策配置
    #[serde(default)]
    pub group_reply: GroupReplyConfig,

    /// 适配器连接配置
    #[serde(default)]
    pub adapter_connection: AdapterConnectionConfig,
}

// ── IdentityConfig ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IdentityConfig {
    /// 助手名称
    #[serde(default = "default_assistant_name")]
    pub name: String,
    /// 助手别名
    #[serde(default)]
    pub alias: String,
    /// 头像路径
    #[serde(default)]
    pub avatar_path: String,
}

fn default_assistant_name() -> String {
    "雪梨".to_string()
}

// ── ModelConfig ──────────────────────────────────────────

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
    /// 上下文窗口大小（token 数）
    #[serde(default = "default_context_window")]
    pub context_window: u32,
    /// 请求超时（秒）
    #[serde(default = "default_timeout")]
    pub timeout: u32,
    /// 响应内容路径
    #[serde(default = "default_response_path")]
    pub response_path: String,
    /// 最大并发请求数
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,
    /// 最大重试次数
    #[serde(default = "default_max_retries")]
    pub max_retries: usize,
    /// 额外请求参数
    #[serde(default)]
    pub extra_params: HashMap<String, serde_json::Value>,
    /// 额外 HTTP 请求头
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
}

fn default_context_window() -> u32 { 128000 }
fn default_timeout() -> u32 { 120 }
fn default_response_path() -> String { "choices.0.message.content".to_string() }
fn default_max_concurrency() -> usize { 5 }
fn default_max_retries() -> usize { 3 }

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            primary_model: "gpt-4o".to_string(),
            light_model: "gpt-4o-mini".to_string(),
            vision_model: Some("gpt-4o".to_string()),
            api_base: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            temperature: 0.7,
            max_tokens: 4096,
            context_window: default_context_window(),
            timeout: default_timeout(),
            response_path: default_response_path(),
            max_concurrency: default_max_concurrency(),
            max_retries: default_max_retries(),
            extra_params: HashMap::new(),
            extra_headers: HashMap::new(),
        }
    }
}

// ── VisionServiceConfig ──────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VisionServiceConfig {
    /// 是否启用
    #[serde(default)]
    pub enabled: bool,
    /// API Base URL（默认使用 model.api_base）
    pub api_base: Option<String>,
    /// API Key（默认使用 model.api_key）
    pub api_key: Option<String>,
    /// 模型名称
    pub model: Option<String>,
    /// 上下文窗口
    #[serde(default = "default_vision_context_window")]
    pub context_window: u32,
    /// 额外请求参数
    #[serde(default)]
    pub extra_params: Option<HashMap<String, serde_json::Value>>,
    /// 额外请求头
    #[serde(default)]
    pub extra_headers: Option<HashMap<String, String>>,
    /// 响应内容路径
    pub response_path: Option<String>,
}

fn default_vision_context_window() -> u32 { 32000 }

impl Default for VisionServiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_base: None,
            api_key: None,
            model: None,
            context_window: default_vision_context_window(),
            extra_params: None,
            extra_headers: None,
            response_path: None,
        }
    }
}

// ── ReplyConfig ──────────────────────────────────────────

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

// ── BotBehaviorConfig ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BotBehaviorConfig {
    /// Token 上下文预算比例
    #[serde(default = "default_token_budget_ratio")]
    pub context_token_budget_ratio: f64,
    /// Token 编码名
    #[serde(default = "default_token_encoding")]
    pub token_encoding: String,
    /// 单条消息最大字符数
    #[serde(default = "default_max_message_length")]
    pub max_message_length: usize,
    /// AI 生成超时（秒）
    #[serde(default = "default_response_timeout")]
    pub response_timeout: u32,
    /// 速率限制：两次回复最小间隔（秒）
    #[serde(default = "default_rate_limit_interval")]
    pub rate_limit_interval: f64,
    /// 是否记录完整 prompt
    #[serde(default)]
    pub log_full_prompt: bool,
    /// 私聊是否使用引用回复
    #[serde(default)]
    pub private_quote_reply_enabled: bool,
    /// 私聊批量窗口（秒）
    #[serde(default = "default_private_batch_window")]
    pub private_batch_window_seconds: f64,
    /// 分段发送总开关
    #[serde(default = "default_segmented_reply_enabled")]
    pub segmented_reply_enabled: bool,
    /// 最大分段数
    #[serde(default = "default_max_segments")]
    pub max_segments: usize,
    /// 首段延迟最小毫秒
    #[serde(default)]
    pub first_segment_delay_min_ms: u32,
    /// 首段延迟最大毫秒
    #[serde(default)]
    pub first_segment_delay_max_ms: u32,
    /// 后续段延迟最小秒
    #[serde(default = "default_followup_delay_min")]
    pub followup_delay_min_seconds: f64,
    /// 后续段延迟最大秒
    #[serde(default = "default_followup_delay_max")]
    pub followup_delay_max_seconds: f64,
    /// 各 AI 用途超时覆盖（秒）
    #[serde(default)]
    pub purpose_timeouts: HashMap<String, f64>,
}

fn default_token_budget_ratio() -> f64 { 0.7 }
fn default_token_encoding() -> String { "cl100k_base".to_string() }
fn default_max_message_length() -> usize { 4000 }
fn default_response_timeout() -> u32 { 60 }
fn default_rate_limit_interval() -> f64 { 1.0 }
fn default_private_batch_window() -> f64 { 1.2 }
fn default_segmented_reply_enabled() -> bool { true }
fn default_max_segments() -> usize { 3 }
fn default_followup_delay_min() -> f64 { 2.0 }
fn default_followup_delay_max() -> f64 { 5.0 }

impl Default for BotBehaviorConfig {
    fn default() -> Self {
        Self {
            context_token_budget_ratio: default_token_budget_ratio(),
            token_encoding: default_token_encoding(),
            max_message_length: default_max_message_length(),
            response_timeout: default_response_timeout(),
            rate_limit_interval: default_rate_limit_interval(),
            log_full_prompt: false,
            private_quote_reply_enabled: false,
            private_batch_window_seconds: default_private_batch_window(),
            segmented_reply_enabled: default_segmented_reply_enabled(),
            max_segments: default_max_segments(),
            first_segment_delay_min_ms: 0,
            first_segment_delay_max_ms: 0,
            followup_delay_min_seconds: default_followup_delay_min(),
            followup_delay_max_seconds: default_followup_delay_max(),
            purpose_timeouts: HashMap::new(),
        }
    }
}

// ── MemoryConfig ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryConfig {
    /// 是否启用
    #[serde(default)]
    pub enabled: bool,
    /// SQLite 数据库路径
    pub db_path: String,
    /// 存储后端
    #[serde(default = "default_storage_backend")]
    pub storage_backend: String,
    /// 记忆提取最小消息数
    pub extraction_min_messages: usize,
    /// BM25 检索返回数
    pub bm25_top_k: usize,
    /// 向量检索返回数
    pub vector_top_k: usize,
    /// Rerank 后返回数
    #[serde(default = "default_rerank_top_k")]
    pub rerank_top_k: usize,
    /// 动态记忆限制
    #[serde(default = "default_dynamic_memory_limit")]
    pub dynamic_memory_limit: usize,
    /// 记忆冲突解决配置
    #[serde(default)]
    pub dispute: MemoryDisputeConfig,
    /// 自动提取
    #[serde(default = "default_auto_extract")]
    pub auto_extract: bool,
    /// 每 N 轮提取一次
    #[serde(default = "default_extract_every_n_turns")]
    pub extract_every_n_turns: usize,
    /// 衰减配置
    #[serde(default)]
    pub decay: MemoryDecayConfig,
    /// 检索权重配置
    #[serde(default)]
    pub retrieval_weights: RetrievalWeightsConfig,
}

fn default_storage_backend() -> String { "sqlite".to_string() }
fn default_rerank_top_k() -> usize { 20 }
fn default_dynamic_memory_limit() -> usize { 8 }
fn default_auto_extract() -> bool { true }
fn default_extract_every_n_turns() -> usize { 3 }

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            db_path: "data/xueli_memory.db".to_string(),
            storage_backend: default_storage_backend(),
            extraction_min_messages: 5,
            bm25_top_k: 10,
            vector_top_k: 5,
            rerank_top_k: default_rerank_top_k(),
            dynamic_memory_limit: default_dynamic_memory_limit(),
            dispute: MemoryDisputeConfig::default(),
            auto_extract: default_auto_extract(),
            extract_every_n_turns: default_extract_every_n_turns(),
            decay: MemoryDecayConfig::default(),
            retrieval_weights: RetrievalWeightsConfig::default(),
        }
    }
}

// ── MemoryDecayConfig ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDecayConfig {
    /// 是否启用普通记忆衰减
    #[serde(default = "default_decay_enabled")]
    pub ordinary_decay_enabled: bool,
    /// 半衰期（天）
    #[serde(default = "default_half_life_days")]
    pub ordinary_half_life_days: f64,
    /// 遗忘阈值
    #[serde(default = "default_forget_threshold")]
    pub ordinary_forget_threshold: f64,
    /// 冷记忆阈值（天）
    #[serde(default = "default_cold_memory_threshold")]
    pub cold_memory_threshold_days: f64,
    /// 冷记忆衰减倍率
    #[serde(default = "default_cold_decay_multiplier")]
    pub cold_decay_multiplier: f64,
}

fn default_decay_enabled() -> bool { true }
fn default_half_life_days() -> f64 { 30.0 }
fn default_forget_threshold() -> f64 { 0.5 }
fn default_cold_memory_threshold() -> f64 { 90.0 }
fn default_cold_decay_multiplier() -> f64 { 1.5 }

impl Default for MemoryDecayConfig {
    fn default() -> Self {
        Self {
            ordinary_decay_enabled: default_decay_enabled(),
            ordinary_half_life_days: default_half_life_days(),
            ordinary_forget_threshold: default_forget_threshold(),
            cold_memory_threshold_days: default_cold_memory_threshold(),
            cold_decay_multiplier: default_cold_decay_multiplier(),
        }
    }
}

// ── RetrievalWeightsConfig ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetrievalWeightsConfig {
    /// BM25 权重
    #[serde(default = "default_bm25_weight")]
    pub local_bm25_weight: f64,
    /// 重要度权重
    #[serde(default = "default_importance_weight")]
    pub local_importance_weight: f64,
    /// 提及权重
    #[serde(default = "default_mention_weight")]
    pub local_mention_weight: f64,
    /// 时效权重
    #[serde(default = "default_recency_weight")]
    pub local_recency_weight: f64,
    /// 场景权重
    #[serde(default = "default_scene_weight")]
    pub local_scene_weight: f64,
    /// 向量权重
    #[serde(default = "default_vector_weight")]
    pub vector_weight: f64,
}

fn default_bm25_weight() -> f64 { 1.0 }
fn default_importance_weight() -> f64 { 0.35 }
fn default_mention_weight() -> f64 { 0.2 }
fn default_recency_weight() -> f64 { 0.15 }
fn default_scene_weight() -> f64 { 0.3 }
fn default_vector_weight() -> f64 { 0.4 }

impl Default for RetrievalWeightsConfig {
    fn default() -> Self {
        Self {
            local_bm25_weight: default_bm25_weight(),
            local_importance_weight: default_importance_weight(),
            local_mention_weight: default_mention_weight(),
            local_recency_weight: default_recency_weight(),
            local_scene_weight: default_scene_weight(),
            vector_weight: default_vector_weight(),
        }
    }
}

// ── MemoryDisputeConfig ──────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDisputeConfig {
    /// 是否启用
    #[serde(default = "default_dispute_enabled")]
    pub enabled: bool,
    /// 高置信度阈值
    #[serde(default = "default_dispute_high_threshold")]
    pub high_confidence_threshold: f64,
    /// 普通置信度阈值
    #[serde(default = "default_dispute_normal_threshold")]
    pub normal_confidence_threshold: f64,
    /// 信号有效期（小时）
    #[serde(default = "default_signal_ttl_hours")]
    pub signal_ttl_hours: f64,
}

fn default_dispute_enabled() -> bool { true }
fn default_dispute_high_threshold() -> f64 { 0.75 }
fn default_dispute_normal_threshold() -> f64 { 0.45 }
fn default_signal_ttl_hours() -> f64 { 168.0 }

impl Default for MemoryDisputeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            high_confidence_threshold: 0.75,
            normal_confidence_threshold: 0.45,
            signal_ttl_hours: default_signal_ttl_hours(),
        }
    }
}

// ── TimingGateConfig ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TimingGateConfig {
    /// 默认主动回复概率
    pub default_proactive_probability: f64,
    /// 被 @ 时回复概率
    pub mention_reply_probability: f64,
}

// ── PlanningWindowConfig ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlanningWindowConfig {
    /// 是否启用
    #[serde(default = "default_planning_window_enabled")]
    pub enabled: bool,
    /// 私聊 wait 窗口时长（秒）
    #[serde(default = "default_private_window_seconds")]
    pub private_window_seconds: f64,
    /// 群聊 wait 窗口时长（秒）
    #[serde(default = "default_group_proactive_window_seconds")]
    pub group_proactive_window_seconds: f64,
    /// 排队消息过期时间
    #[serde(default = "default_queue_expire_seconds")]
    pub queue_expire_seconds: f64,
}

fn default_planning_window_enabled() -> bool { true }
fn default_private_window_seconds() -> f64 { 1.2 }
fn default_group_proactive_window_seconds() -> f64 { 0.45 }
fn default_queue_expire_seconds() -> f64 { 60.0 }

impl Default for PlanningWindowConfig {
    fn default() -> Self {
        Self {
            enabled: default_planning_window_enabled(),
            private_window_seconds: default_private_window_seconds(),
            group_proactive_window_seconds: default_group_proactive_window_seconds(),
            queue_expire_seconds: default_queue_expire_seconds(),
        }
    }
}

// ── SessionConfig ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionConfig {
    /// 会话超时时间（秒）
    pub session_timeout_secs: u64,
    /// 每会话最大并发消息数
    pub max_concurrent_messages: usize,
}

// ── EmojiConfig ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmojiConfig {
    /// 是否启用表情系统
    pub enabled: bool,
    /// 表情包数据库路径
    pub db_path: Option<String>,
    /// 是否启用捕获
    #[serde(default = "default_emoji_capture_enabled")]
    pub capture_enabled: bool,
    /// 是否启用分类
    #[serde(default = "default_emoji_classification_enabled")]
    pub classification_enabled: bool,
    /// 分类前空闲秒数
    #[serde(default = "default_emoji_idle_seconds")]
    pub idle_seconds_before_classify: f64,
    /// 分类间隔秒数
    #[serde(default = "default_emoji_classification_interval")]
    pub classification_interval_seconds: f64,
    /// 回复是否启用表情
    #[serde(default)]
    pub reply_enabled: bool,
    /// 表情回复冷却秒数
    #[serde(default = "default_emoji_reply_cooldown")]
    pub reply_cooldown_seconds: f64,
    /// 最大存储表情数
    #[serde(default = "default_max_stored_emojis")]
    pub max_stored_emojis: usize,
    /// 溢出策略
    #[serde(default = "default_emoji_overflow_policy")]
    pub overflow_policy: String,
}

fn default_emoji_capture_enabled() -> bool { true }
fn default_emoji_classification_enabled() -> bool { true }
fn default_emoji_idle_seconds() -> f64 { 45.0 }
fn default_emoji_classification_interval() -> f64 { 30.0 }
fn default_emoji_reply_cooldown() -> f64 { 180.0 }
fn default_max_stored_emojis() -> usize { 100 }
fn default_emoji_overflow_policy() -> String { "replace_oldest".to_string() }

impl Default for EmojiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            db_path: None,
            capture_enabled: default_emoji_capture_enabled(),
            classification_enabled: default_emoji_classification_enabled(),
            idle_seconds_before_classify: default_emoji_idle_seconds(),
            classification_interval_seconds: default_emoji_classification_interval(),
            reply_enabled: false,
            reply_cooldown_seconds: default_emoji_reply_cooldown(),
            max_stored_emojis: default_max_stored_emojis(),
            overflow_policy: default_emoji_overflow_policy(),
        }
    }
}

// ── ProactiveShareConfig ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProactiveShareConfig {
    /// 是否启用主动分享
    pub enabled: bool,
    /// 空闲后触发（小时）
    #[serde(default = "default_idle_hours")]
    pub idle_hours: f64,
    /// 冷却时间（小时）
    #[serde(default = "default_cooldown_hours")]
    pub cooldown_hours: f64,
    /// 每日最大发送量
    #[serde(default = "default_max_per_day")]
    pub max_per_day: usize,
    /// 时间范围起始
    #[serde(default = "default_time_range_start")]
    pub time_range_start: String,
    /// 时间范围结束
    #[serde(default = "default_time_range_end")]
    pub time_range_end: String,
    /// 触发来源
    #[serde(default = "default_trigger_sources")]
    pub trigger_sources: Vec<String>,
    /// 分享间隔（秒）
    pub interval_secs: u64,
}

fn default_idle_hours() -> f64 { 24.0 }
fn default_cooldown_hours() -> f64 { 6.0 }
fn default_max_per_day() -> usize { 3 }
fn default_time_range_start() -> String { "09:00".to_string() }
fn default_time_range_end() -> String { "22:00".to_string() }
fn default_trigger_sources() -> Vec<String> { vec!["insight".to_string(), "time_signal".to_string()] }

impl Default for ProactiveShareConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            idle_hours: default_idle_hours(),
            cooldown_hours: default_cooldown_hours(),
            max_per_day: default_max_per_day(),
            time_range_start: default_time_range_start(),
            time_range_end: default_time_range_end(),
            trigger_sources: default_trigger_sources(),
            interval_secs: 3600,
        }
    }
}

// ── CharacterGrowthConfig ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CharacterGrowthConfig {
    /// 是否启用
    #[serde(default = "default_growth_enabled")]
    pub enabled: bool,
    /// 情绪波动总开关
    #[serde(default = "default_mood_fluctuation_enabled")]
    pub mood_fluctuation_enabled: bool,
    /// 情绪波动幅度
    #[serde(default = "default_mood_volatility")]
    pub mood_volatility: f64,
    /// 情绪自主性
    #[serde(default = "default_mood_independence_ratio")]
    pub mood_independence_ratio: f64,
    /// 每轮能量衰减
    #[serde(default = "default_mood_energy_decay")]
    pub mood_energy_decay_per_turn: f64,
    /// 夜间能量恢复
    #[serde(default = "default_mood_energy_recovery")]
    pub mood_energy_recovery_night: f64,
    /// 情绪回归基线速率
    #[serde(default = "default_mood_valence_decay")]
    pub mood_valence_decay_rate: f64,
    /// 夜间逻辑增长恢复速率
    #[serde(default = "default_mood_recovery_rate")]
    pub mood_recovery_rate: f64,
    /// 情绪周期（天）
    #[serde(default = "default_mood_cycle_length")]
    pub mood_cycle_length_days: usize,
    /// 是否在回复中显示情绪
    #[serde(default = "default_mood_show_in_reply")]
    pub mood_show_in_reply: bool,
    /// 关系追踪开关
    #[serde(default = "default_relationship_tracking")]
    pub relationship_tracking_enabled: bool,
}

fn default_growth_enabled() -> bool { true }
fn default_mood_fluctuation_enabled() -> bool { true }
fn default_mood_volatility() -> f64 { 0.3 }
fn default_mood_independence_ratio() -> f64 { 0.7 }
fn default_mood_energy_decay() -> f64 { 0.05 }
fn default_mood_energy_recovery() -> f64 { 0.2 }
fn default_mood_valence_decay() -> f64 { 0.15 }
fn default_mood_recovery_rate() -> f64 { 0.4 }
fn default_mood_cycle_length() -> usize { 7 }
fn default_mood_show_in_reply() -> bool { true }
fn default_relationship_tracking() -> bool { true }

impl Default for CharacterGrowthConfig {
    fn default() -> Self {
        Self {
            enabled: default_growth_enabled(),
            mood_fluctuation_enabled: default_mood_fluctuation_enabled(),
            mood_volatility: default_mood_volatility(),
            mood_independence_ratio: default_mood_independence_ratio(),
            mood_energy_decay_per_turn: default_mood_energy_decay(),
            mood_energy_recovery_night: default_mood_energy_recovery(),
            mood_valence_decay_rate: default_mood_valence_decay(),
            mood_recovery_rate: default_mood_recovery_rate(),
            mood_cycle_length_days: default_mood_cycle_length(),
            mood_show_in_reply: default_mood_show_in_reply(),
            relationship_tracking_enabled: default_relationship_tracking(),
        }
    }
}

// ── GroupReplyConfig ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GroupReplyConfig {
    /// 仅当被 @ 时回复
    #[serde(default = "default_only_reply_when_at")]
    pub only_reply_when_at: bool,
    /// 兴趣回复开关
    #[serde(default = "default_interest_reply_enabled")]
    pub interest_reply_enabled: bool,
    /// 规划请求间隔
    #[serde(default = "default_plan_request_interval")]
    pub plan_request_interval: f64,
    /// 规划请求最大并行数
    #[serde(default = "default_plan_request_max_parallel")]
    pub plan_request_max_parallel: usize,
    /// 主动回复时 @ 用户
    #[serde(default)]
    pub at_user_when_proactive_reply: bool,
    /// 群聊 wait 窗口（秒）
    #[serde(default = "default_group_wait_window")]
    pub group_wait_window_seconds: f64,
    /// 回复概率
    #[serde(default = "default_reply_probability")]
    pub reply_probability: f64,
    /// 触发阈值
    #[serde(default = "default_trigger_threshold")]
    pub trigger_threshold: usize,
    /// 空闲宽限（秒）
    #[serde(default = "default_idle_grace_seconds")]
    pub idle_grace_seconds: f64,
    /// 防抖秒数
    #[serde(default = "default_debounce_seconds")]
    pub debounce_seconds: f64,
    /// 防抖最大重置次数
    #[serde(default = "default_debounce_max_resets")]
    pub debounce_max_resets: usize,
    /// 基础频率
    #[serde(default = "default_base_frequency")]
    pub base_frequency: f64,
    /// 时间规则
    #[serde(default)]
    pub time_rules: HashMap<String, f64>,
    /// 群组覆盖
    #[serde(default)]
    pub group_overrides: HashMap<String, f64>,
    /// 饱和对数因子
    #[serde(default = "default_saturation_log_factor")]
    pub saturation_log_factor: f64,
}

fn default_only_reply_when_at() -> bool { true }
fn default_interest_reply_enabled() -> bool { true }
fn default_plan_request_interval() -> f64 { 3.0 }
fn default_plan_request_max_parallel() -> usize { 1 }
fn default_group_wait_window() -> f64 { 5.0 }
fn default_reply_probability() -> f64 { 1.0 }
fn default_trigger_threshold() -> usize { 1 }
fn default_idle_grace_seconds() -> f64 { 300.0 }
fn default_debounce_seconds() -> f64 { 1.0 }
fn default_debounce_max_resets() -> usize { 5 }
fn default_base_frequency() -> f64 { 1.0 }
fn default_saturation_log_factor() -> f64 { 1.2 }

impl Default for GroupReplyConfig {
    fn default() -> Self {
        Self {
            only_reply_when_at: default_only_reply_when_at(),
            interest_reply_enabled: default_interest_reply_enabled(),
            plan_request_interval: default_plan_request_interval(),
            plan_request_max_parallel: default_plan_request_max_parallel(),
            at_user_when_proactive_reply: false,
            group_wait_window_seconds: default_group_wait_window(),
            reply_probability: default_reply_probability(),
            trigger_threshold: default_trigger_threshold(),
            idle_grace_seconds: default_idle_grace_seconds(),
            debounce_seconds: default_debounce_seconds(),
            debounce_max_resets: default_debounce_max_resets(),
            base_frequency: default_base_frequency(),
            time_rules: HashMap::new(),
            group_overrides: HashMap::new(),
            saturation_log_factor: default_saturation_log_factor(),
        }
    }
}

// ── AdapterConnectionConfig ──────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AdapterConnectionConfig {
    /// 适配器类型
    #[serde(default)]
    pub adapter: String,
    /// 平台标识
    #[serde(default)]
    pub platform: String,
    /// WebSocket 地址
    #[serde(default = "default_ws_url")]
    pub ws_url: String,
    /// HTTP 地址
    #[serde(default = "default_http_url")]
    pub http_url: String,
}

fn default_ws_url() -> String { "ws://0.0.0.0:8095".to_string() }
fn default_http_url() -> String { "http://127.0.0.1:6700".to_string() }

impl Default for AdapterConnectionConfig {
    fn default() -> Self {
        Self {
            adapter: String::new(),
            platform: String::new(),
            ws_url: default_ws_url(),
            http_url: default_http_url(),
        }
    }
}

// ── XueliConfig 默认值实现 ───────────────────────────────

impl Default for XueliConfig {
    fn default() -> Self {
        Self {
            model: ModelConfig::default(),
            vision: VisionServiceConfig::default(),
            reply: ReplyConfig {
                group_cooldown_secs: 10.0,
                private_cooldown_secs: 3.0,
                max_context_messages: 50,
                max_reply_chars: 500,
            },
            bot_behavior: BotBehaviorConfig::default(),
            memory: MemoryConfig::default(),
            timing_gate: TimingGateConfig {
                default_proactive_probability: 0.3,
                mention_reply_probability: 0.95,
            },
            planning_window: PlanningWindowConfig::default(),
            session: SessionConfig {
                session_timeout_secs: 3600,
                max_concurrent_messages: 10,
            },
            emoji: EmojiConfig::default(),
            proactive_share: ProactiveShareConfig::default(),
            identity: IdentityConfig {
                name: "雪梨".to_string(),
                alias: String::new(),
                avatar_path: String::new(),
            },
            character_growth: CharacterGrowthConfig::default(),
            memory_dispute: MemoryDisputeConfig::default(),
            group_reply: GroupReplyConfig::default(),
            adapter_connection: AdapterConnectionConfig::default(),
        }
    }
}

impl XueliConfig {
    /// 从 TOML 文件加载配置
    pub fn from_file(path: &str) -> XueliResult<Self> {
        let content =
            std::fs::read_to_string(path).map_err(|e| crate::core::errors::XueliError::Config(format!("读取配置文件失败: {}", e)))?;
        toml::from_str(&content).map_err(|e| crate::core::errors::XueliError::Config(format!("解析 TOML 配置失败: {}", e)).into())
    }

    /// 使用 config-rs 从多源加载配置（环境变量 + 文件）
    pub fn from_sources(config_file: Option<&str>) -> XueliResult<Self> {
        let mut builder = config::Config::builder();

        // 1. 从文件加载（作为基础）
        if let Some(path) = config_file {
            builder = builder.add_source(config::File::with_name(path).required(false));
        }

        // 2. 环境变量覆盖（XUELI_ 前缀，双下划线分隔层级）
        // 例如：XUELI__MODEL__API_KEY=xxx 会覆盖 model.api_key
        builder = builder.add_source(
            config::Environment::with_prefix("XUELI")
                .separator("__")
                .try_parsing(true),
        );

        let cfg = builder
            .build()
            .map_err(|e| crate::core::errors::XueliError::Config(format!("构建配置失败: {}", e)))?;

        cfg.try_deserialize()
            .map_err(|e| crate::core::errors::XueliError::Config(format!("反序列化配置失败: {}", e)).into())
    }

    /// 检查 AI 服务是否已配置
    pub fn is_ai_service_configured(&self) -> bool {
        !self.model.api_base.is_empty() && !self.model.primary_model.is_empty()
    }

    /// 检查视觉服务是否已配置
    pub fn is_vision_service_configured(&self) -> bool {
        if !self.vision.enabled {
            return false;
        }
        let base = self.vision.api_base.as_ref().unwrap_or(&self.model.api_base);
        let model = self.vision.model.as_ref().unwrap_or(&self.model.primary_model);
        !base.is_empty() && !model.is_empty()
    }

    /// 获取视觉服务状态
    pub fn vision_service_status(&self) -> &'static str {
        if !self.vision.enabled {
            return "disabled";
        }
        if self.is_vision_service_configured() {
            "enabled"
        } else {
            "unconfigured"
        }
    }
}
