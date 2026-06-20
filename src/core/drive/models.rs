//! 内驱力系统数据模型 — 三层状态、事件规则、反思输出。
//!
//! 三层结构：
//!   情绪层 (Affective): PAD 向量 — valence/arousal/dominance
//!   动机层 (Motivational): baseline + 瞬时_offset 双层结构
//!   关系层 (Relational): intimacy/trust/attention_weight，按作用域隔离

use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

// ─── 谨慎度阈值常量 ─────────────────────────────────────

/// caution 动机值 >= 此值视为 medium
pub const CAUTION_THRESHOLD_MEDIUM: f64 = 0.4;
/// caution 动机值 >= 此值视为 high
pub const CAUTION_THRESHOLD_HIGH: f64 = 0.7;

/// 从 caution 动机值映射为谨慎级别字符串。
pub fn caution_level_from_value(caution_val: f64) -> &'static str {
    if caution_val >= CAUTION_THRESHOLD_HIGH {
        "high"
    } else if caution_val >= CAUTION_THRESHOLD_MEDIUM {
        "medium"
    } else {
        "low"
    }
}

// ─── 情绪层 ─────────────────────────────────────────────

/// 情绪层 PAD 向量：愉悦度、唤醒度、支配度。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PADVector {
    /// 愉悦度 [-1, 1]
    #[serde(default)]
    pub valence: f64,
    /// 唤醒度 [0, 1]
    #[serde(default = "default_arousal")]
    pub arousal: f64,
    /// 支配度 [0, 1]
    #[serde(default = "default_dominance")]
    pub dominance: f64,
}

fn default_arousal() -> f64 {
    0.5
}
fn default_dominance() -> f64 {
    0.5
}

impl Default for PADVector {
    fn default() -> Self {
        Self {
            valence: 0.0,
            arousal: default_arousal(),
            dominance: default_dominance(),
        }
    }
}

impl PADVector {
    /// 返回钳制到有效边界后的副本。
    pub fn clamp(&self) -> PADVector {
        PADVector {
            valence: self.valence.clamp(-1.0, 1.0),
            arousal: self.arousal.clamp(0.0, 1.0),
            dominance: self.dominance.clamp(0.0, 1.0),
        }
    }

    /// 是否全为零增量
    pub fn is_zero(&self) -> bool {
        self.valence == 0.0 && self.arousal == 0.0 && self.dominance == 0.0
    }
}

/// 情绪层完整状态，含 PAD 向量和元信息。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AffectiveState {
    #[serde(default)]
    pub pad: PADVector,
    #[serde(default)]
    pub updated_at: String,
}

// ─── 动机层 ─────────────────────────────────────────────

/// 动机维度键名。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MotivationalKey {
    /// 社交需求
    SocialDrive,
    /// 信息好奇
    Curiosity,
    /// 安全谨慎
    Caution,
    /// 主动性
    Proactivity,
    /// 表达欲/倾诉欲
    Expressiveness,
    /// 归属感/群体认同
    Belonging,
}

impl MotivationalKey {
    /// 所有动机维度键
    pub fn all() -> &'static [MotivationalKey] {
        &[
            MotivationalKey::SocialDrive,
            MotivationalKey::Curiosity,
            MotivationalKey::Caution,
            MotivationalKey::Proactivity,
            MotivationalKey::Expressiveness,
            MotivationalKey::Belonging,
        ]
    }

    /// 序列化键名（与 Python 版 key.value 对应）
    pub fn as_str(&self) -> &'static str {
        match self {
            MotivationalKey::SocialDrive => "social_drive",
            MotivationalKey::Curiosity => "curiosity",
            MotivationalKey::Caution => "caution",
            MotivationalKey::Proactivity => "proactivity",
            MotivationalKey::Expressiveness => "expressiveness",
            MotivationalKey::Belonging => "belonging",
        }
    }
}

/// 动机维度：基线值 + 瞬时偏移的双层结构。
///
/// effective = clamp(baseline + transient_offset, 0, 1)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotivationalDimension {
    /// LLM 反思更新，缓慢变化 [0, 1]
    #[serde(default = "default_baseline")]
    pub baseline: f64,
    /// 事件触发，快速衰减 [-1, 1]
    #[serde(default)]
    pub transient_offset: f64,
    /// 半衰期衰减率（每 tick 衰减比例），可由反思调整
    #[serde(default = "default_decay_rate")]
    pub decay_rate: f64,
}

fn default_baseline() -> f64 {
    0.5
}
fn default_decay_rate() -> f64 {
    0.1
}

impl Default for MotivationalDimension {
    fn default() -> Self {
        Self {
            baseline: default_baseline(),
            transient_offset: 0.0,
            decay_rate: default_decay_rate(),
        }
    }
}

impl MotivationalDimension {
    /// 对外暴露的有效值。
    pub fn effective(&self) -> f64 {
        (self.baseline + self.transient_offset).clamp(0.0, 1.0)
    }

    /// 按 decay_rate 衰减瞬时偏移，趋向归零。
    pub fn apply_decay(&mut self) {
        self.transient_offset *= 1.0 - self.decay_rate;
        if self.transient_offset.abs() < 0.001 {
            self.transient_offset = 0.0;
        }
    }
}

// ─── 关系层 ─────────────────────────────────────────────

/// 关系层状态：按作用域隔离的用户级关系向量。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RelationalState {
    /// 亲密度 [0, 1]
    #[serde(default)]
    pub intimacy: f64,
    /// 信任度 [0, 1]
    #[serde(default = "default_trust")]
    pub trust: f64,
    /// 关注权重 [0, 1]
    #[serde(default = "default_attention")]
    pub attention_weight: f64,
    #[serde(default)]
    pub updated_at: String,
}

fn default_trust() -> f64 {
    0.5
}
fn default_attention() -> f64 {
    0.5
}

// ─── 事件规则 ───────────────────────────────────────────

/// 事件增量规则：匹配事件特征，产生动机层/情绪层瞬时偏移。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRule {
    /// 规则唯一标识
    pub rule_id: String,
    /// 事件匹配模式
    pub event_pattern: String,
    /// 情绪层增量
    #[serde(default)]
    pub affective_deltas: PADVector,
    /// 动机层增量 {key: delta}
    #[serde(default)]
    pub motivational_deltas: HashMap<String, f64>,
    /// 规则权重，可由反思调整
    #[serde(default = "default_weight")]
    pub weight: f64,
    /// 触发时面向 LLM 的回复指导
    #[serde(default)]
    pub guidance: String,
    /// 规则描述
    #[serde(default)]
    pub description: String,
}

fn default_weight() -> f64 {
    1.0
}

/// 事件增量规则集，支持 LLM 反思动态调整权重。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRuleSet {
    #[serde(default)]
    pub rules: Vec<EventRule>,
    /// 规则集版本号
    #[serde(default = "default_version")]
    pub version: i64,
}

fn default_version() -> i64 {
    1
}

impl Default for EventRuleSet {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            version: 1,
        }
    }
}

impl EventRuleSet {
    /// 返回匹配指定模式的所有规则。
    pub fn match_pattern(&self, event_pattern: &str) -> Vec<&EventRule> {
        self.rules
            .iter()
            .filter(|r| r.event_pattern == event_pattern)
            .collect()
    }
}

// ─── 反思输出 ───────────────────────────────────────────

/// 单条规则权重/衰减率调整建议。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleWeightAdjustment {
    pub rule_id: String,
    /// None 表示不调整
    pub new_weight: Option<f64>,
    /// None 表示不调整
    pub new_decay_rate: Option<f64>,
    #[serde(default)]
    pub reason: String,
}

/// LLM 反思输出：基线更新 + 规则权重调整 + 规则集替换。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionOutput {
    /// 动机层基线更新 {motivational_key: new_baseline}
    #[serde(default)]
    pub baseline_updates: HashMap<String, f64>,
    /// 情绪层基线偏移（向中性回归的微调）
    #[serde(default)]
    pub affective_baseline_shift: PADVector,
    /// 规则权重/衰减率调整
    #[serde(default)]
    pub rule_adjustments: Vec<RuleWeightAdjustment>,
    /// 完整规则集替换（None 表示不替换，仅做权重调整）
    pub new_rule_set: Option<EventRuleSet>,
    /// 反思摘要
    #[serde(default)]
    pub summary: String,
    /// 置信度
    #[serde(default)]
    pub confidence: f64,
}

// ─── 统一快照与上下文 ───────────────────────────────────

/// 内驱力系统完整状态快照，用于持久化和恢复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveSnapshot {
    #[serde(default)]
    pub affective: AffectiveState,
    /// 动机层 {key: dimension}，key 为 MotivationalKey::as_str()
    #[serde(default)]
    pub motivational: HashMap<String, MotivationalDimension>,
    /// 关系层 {scope_key:user_id: state}
    #[serde(default)]
    pub relational: HashMap<String, RelationalState>,
    #[serde(default)]
    pub event_rules: EventRuleSet,
    /// 作用域键
    #[serde(default)]
    pub scope_key: String,
    /// 快照格式版本
    #[serde(default = "default_version")]
    pub version: i64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

impl Default for DriveSnapshot {
    fn default() -> Self {
        Self::create_default("")
    }
}

impl DriveSnapshot {
    /// 创建默认初始状态快照。
    pub fn create_default(scope_key: &str) -> DriveSnapshot {
        let now = Utc::now().to_rfc3339();
        let mut motivational = HashMap::new();
        for key in MotivationalKey::all() {
            motivational.insert(key.as_str().to_string(), MotivationalDimension::default());
        }
        DriveSnapshot {
            affective: AffectiveState {
                pad: PADVector::default(),
                updated_at: now.clone(),
            },
            motivational,
            relational: HashMap::new(),
            event_rules: EventRuleSet::default(),
            scope_key: scope_key.to_string(),
            version: 1,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

/// 统一注入用的内驱力上下文，供 context_builder 消费。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveContext {
    #[serde(default)]
    pub affective: PADVector,
    /// {key: effective_value}
    #[serde(default)]
    pub motivational: HashMap<String, f64>,
    #[serde(default)]
    pub relational: RelationalState,
    /// 谨慎度相关的回复指导
    #[serde(default)]
    pub caution_guidance: Vec<String>,
    /// 当前活跃的事件模式
    #[serde(default)]
    pub active_event_patterns: Vec<String>,
    /// 动态记忆上下文（会话恢复/精准回忆/相关记忆拼接）
    #[serde(default)]
    pub memory_context: String,
    #[serde(default)]
    pub scope_key: String,
    #[serde(default)]
    pub user_id: String,
}

impl Default for DriveContext {
    fn default() -> Self {
        Self {
            affective: PADVector::default(),
            motivational: HashMap::new(),
            relational: RelationalState::default(),
            caution_guidance: Vec::new(),
            active_event_patterns: Vec::new(),
            memory_context: String::new(),
            scope_key: String::new(),
            user_id: String::new(),
        }
    }
}

impl DriveContext {
    /// 生成面向 LLM 的内驱力状态文本段落。
    pub fn to_prompt_section(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        lines.push("【内驱力状态】".to_string());
        lines.push(format!(
            "情绪: 愉悦度={:.2} 唤醒度={:.2} 支配度={:.2}",
            self.affective.valence, self.affective.arousal, self.affective.dominance
        ));

        let mot_parts: Vec<String> = self
            .motivational
            .iter()
            .map(|(k, v)| format!("{}={:.2}", k, v))
            .collect();
        if !mot_parts.is_empty() {
            lines.push(format!("动机: {}", mot_parts.join(" ")));
        }

        if !self.user_id.is_empty() {
            lines.push(format!(
                "关系: 亲密度={:.2} 信任度={:.2} 关注={:.2}",
                self.relational.intimacy, self.relational.trust, self.relational.attention_weight
            ));
        }

        // 动态记忆上下文
        if !self.memory_context.is_empty() {
            lines.push(format!("记忆上下文:\n{}", self.memory_context.trim()));
        }

        // 活跃事件模式
        if !self.active_event_patterns.is_empty() {
            // 去重保持顺序
            let mut seen = std::collections::HashSet::new();
            let mut unique_patterns: Vec<&str> = Vec::new();
            for p in &self.active_event_patterns {
                if seen.insert(p.as_str()) {
                    unique_patterns.push(p);
                }
            }
            lines.push(format!("活跃事件模式: {}", unique_patterns.join(", ")));
        }

        // 谨慎度指导
        if !self.caution_guidance.is_empty() {
            // 去重保持顺序
            let mut seen = std::collections::HashSet::new();
            let mut unique_guidance: Vec<&str> = Vec::new();
            for g in &self.caution_guidance {
                if seen.insert(g.as_str()) {
                    unique_guidance.push(g);
                }
            }
            let guidance_text = unique_guidance.join("；");
            let caution_val = self.motivational.get("caution").copied().unwrap_or(0.0);
            let level = caution_level_from_value(caution_val);
            if level != "low" {
                lines.push(format!("谨慎度=级别={}", level));
                lines.push(format!("回复要求={}", guidance_text));
            }
        }

        lines.join("\n")
    }
}

/// 事件日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLogEntry {
    pub pattern: String,
    pub timestamp: String,
}
