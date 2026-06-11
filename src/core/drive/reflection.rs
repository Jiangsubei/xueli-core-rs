//! DriveReflection — LLM 反思调度与 prompt 拼接。
//!
//! 职责：
//!   - 收集近期事件日志、状态快照、规则版本
//!   - 拼接反思 prompt
//!   - 调用 LLM 获取反思结果
//!   - 解析为 ReflectionOutput

use std::collections::HashMap;
use std::sync::Arc;

use regex::Regex;
use serde_json::Value;
use tracing::{debug, warn};

use crate::services::prompt_loader::FilePromptTemplateLoader;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};

use super::models::{
    DriveSnapshot, EventLogEntry, EventRuleSet, PADVector, ReflectionOutput, RuleWeightAdjustment,
};

const REFLECTION_PROMPT_TEMPLATE: &str = "drive_reflection";

/// 提示词模板加载器的动态分发包装。
/// PromptTemplateLoader 使用 impl Future 返回类型，不是 dyn-compatible，
/// 因此我们通过此包装器将方法封装为 owned future。
pub trait DynTemplateLoader: Send + Sync {
    fn get_template_boxed(
        &self,
        locale: String,
        name: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, crate::core::errors::XueliError>> + Send>>;
}

/// 为 FilePromptTemplateLoader 实现 DynTemplateLoader。
/// 通过 cache_arc() 获取 owned 缓存 Arc，避免 async 块中捕获 &self。
impl DynTemplateLoader for FilePromptTemplateLoader {
    fn get_template_boxed(
        &self,
        locale: String,
        name: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, crate::core::errors::XueliError>> + Send>> {
        let cache = self.cache_arc();
        Box::pin(async move {
            // 先检查缓存
            {
                let cache_read = cache.read().await;
                if let Some(cached) = cache_read.get(&locale) {
                    if let Some(template) = cached.get(&name) {
                        return Ok(template.clone());
                    }
                }
            }
            Err(crate::core::errors::XueliError::Template(
                crate::core::errors::TemplateError::NotFound(format!("{} / {}", locale, name)),
            ))
        })
    }
}

/// LLM 反思调度器。
pub struct DriveReflection {
    ai_client: Option<Arc<dyn AIClient>>,
    template_loader: Option<Arc<dyn DynTemplateLoader>>,
    max_rule_weight_adjustment: f64,
}

impl DriveReflection {
    pub fn new(
        ai_client: Option<Arc<dyn AIClient>>,
        template_loader: Option<Arc<dyn DynTemplateLoader>>,
        max_rule_weight_adjustment: f64,
    ) -> Self {
        Self {
            ai_client,
            template_loader,
            max_rule_weight_adjustment,
        }
    }

    /// 便捷构造：使用 FilePromptTemplateLoader
    pub fn with_file_loader(
        ai_client: Option<Arc<dyn AIClient>>,
        template_base_dir: &str,
        max_rule_weight_adjustment: f64,
    ) -> Self {
        let loader: Arc<FilePromptTemplateLoader> =
            Arc::new(FilePromptTemplateLoader::new(template_base_dir));
        Self {
            ai_client,
            template_loader: Some(loader),
            max_rule_weight_adjustment,
        }
    }

    /// 执行一次 LLM 反思。
    pub async fn run_reflection(
        &self,
        snapshot: &DriveSnapshot,
        event_log: &[EventLogEntry],
        round_count: usize,
    ) -> Option<ReflectionOutput> {
        let ai_client = self.ai_client.as_ref()?;

        // 加载模板
        let system_prompt: String = match &self.template_loader {
            Some(loader) => match loader.get_template_boxed("zh-CN".to_string(), REFLECTION_PROMPT_TEMPLATE.to_string()).await {
                Ok(t) => t,
                Err(_) => {
                    warn!(
                        "[DriveReflection] 模板文件 {} 不存在，跳过反思",
                        REFLECTION_PROMPT_TEMPLATE
                    );
                    return None;
                }
            },
            None => {
                warn!("[DriveReflection] 无模板加载器，跳过反思");
                return None;
            }
        };

        let user_content = self.build_user_prompt(snapshot, event_log, round_count);

        let data = match self.invoke_llm(ai_client.as_ref(), &system_prompt, &user_content).await {
            Ok(d) => d,
            Err(e) => {
                debug!("[DriveReflection] LLM 调用失败: {}", e);
                return None;
            }
        };

        Some(self.parse_reflection_output(&data))
    }

    /// 拼接反思 prompt 的用户消息部分。
    fn build_user_prompt(
        &self,
        snapshot: &DriveSnapshot,
        event_log: &[EventLogEntry],
        round_count: usize,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();

        // 当前状态
        parts.push("【当前内驱力状态】".to_string());
        let pad = &snapshot.affective.pad;
        parts.push(format!(
            "情绪层: 愉悦度={:.3} 唤醒度={:.3} 支配度={:.3}",
            pad.valence, pad.arousal, pad.dominance
        ));
        for (key, dim) in &snapshot.motivational {
            parts.push(format!(
                "动机层.{}: baseline={:.3} offset={:+.3} effective={:.3}",
                key,
                dim.baseline,
                dim.transient_offset,
                dim.effective()
            ));
        }
        if !snapshot.relational.is_empty() {
            parts.push("关系层:".to_string());
            for (user_key, rel) in &snapshot.relational {
                parts.push(format!(
                    "  {}: intimacy={:.3} trust={:.3}",
                    user_key, rel.intimacy, rel.trust
                ));
            }
        }

        // 近期事件
        if !event_log.is_empty() {
            parts.push(format!("\n【近期事件日志】(共 {} 条)", event_log.len()));
            for entry in event_log.iter().rev().take(20) {
                parts.push(format!("  [{}] {}", entry.timestamp, entry.pattern));
            }
        }

        // 规则版本
        parts.push(format!(
            "\n【规则集版本】v{}",
            snapshot.event_rules.version
        ));
        for rule in &snapshot.event_rules.rules {
            parts.push(format!(
                "  {}: weight={:.2} pattern={}",
                rule.rule_id, rule.weight, rule.event_pattern
            ));
        }

        parts.push(format!(
            "\n【统计】自上次反思以来的对话轮次: {}",
            round_count
        ));

        parts.join("\n")
    }

    /// 调用 LLM 并返回解析后的 JSON。
    async fn invoke_llm(
        &self,
        ai_client: &dyn AIClient,
        system_prompt: &str,
        user_content: &str,
    ) -> Result<Value, String> {
        let messages = vec![
            ChatMessage::text("system", system_prompt),
            ChatMessage::text("user", user_content),
        ];

        let request = ChatCompletionRequest {
            model: String::new(), // 由 AIClient 实现决定
            messages,
            temperature: Some(0.2),
            max_tokens: None,
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: HashMap::new(),
        };

        let response = ai_client
            .chat_completion(&request)
            .await
            .map_err(|e| e.to_string())?;

        Ok(Self::extract_json(&response.content))
    }

    /// 从 LLM 回复中提取 JSON 对象。
    fn extract_json(content: &str) -> Value {
        let text = content.trim();
        if text.is_empty() {
            return Value::Null;
        }

        // 直接解析
        if let Ok(v) = serde_json::from_str::<Value>(text) {
            if v.is_object() {
                return v;
            }
        }

        // fenced code block
        let re_fenced = Regex::new(r"(?is)```(?:json)?\s*(\{.*?\})\s*```").unwrap();
        if let Some(caps) = re_fenced.captures(text) {
            if let Some(m) = caps.get(1) {
                if let Ok(v) = serde_json::from_str::<Value>(m.as_str()) {
                    if v.is_object() {
                        return v;
                    }
                }
            }
        }

        // 任意 JSON 对象
        let re_json = Regex::new(r"(?s)\{.*\}").unwrap();
        if let Some(m) = re_json.find(text) {
            if let Ok(v) = serde_json::from_str::<Value>(m.as_str()) {
                if v.is_object() {
                    return v;
                }
            }
        }

        Value::Null
    }

    /// 将 LLM 返回的 JSON 解析为 ReflectionOutput。
    fn parse_reflection_output(&self, data: &Value) -> ReflectionOutput {
        // 基线更新
        let mut baseline_updates: HashMap<String, f64> = HashMap::new();
        let raw_baselines = data
            .get("baseline_updates")
            .or_else(|| data.get("baselines"))
            .and_then(|v| v.as_object());
        if let Some(obj) = raw_baselines {
            for (k, v) in obj {
                if let Some(n) = v.as_f64() {
                    baseline_updates.insert(k.clone(), n.clamp(0.0, 1.0));
                }
            }
        }

        // 情绪层基线偏移
        let raw_aff = data
            .get("affective_baseline_shift")
            .or_else(|| data.get("affective_shift"))
            .and_then(|v| v.as_object());
        let affective_shift = if let Some(obj) = raw_aff {
            PADVector {
                valence: obj
                    .get("valence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                arousal: obj
                    .get("arousal")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                dominance: obj
                    .get("dominance")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
            }
        } else {
            PADVector::default()
        };

        // 规则权重调整
        let mut rule_adjustments: Vec<RuleWeightAdjustment> = Vec::new();
        let raw_adj = data
            .get("rule_adjustments")
            .or_else(|| data.get("rule_weight_adjustments"))
            .and_then(|v| v.as_array());
        if let Some(arr) = raw_adj {
            for item in arr {
                let obj = match item.as_object() {
                    Some(o) => o,
                    None => continue,
                };
                let rule_id = match obj.get("rule_id").and_then(|v| v.as_str()) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => continue,
                };
                let new_weight = obj.get("new_weight").and_then(|v| v.as_f64());
                let new_decay_rate = obj.get("new_decay_rate").and_then(|v| v.as_f64());
                let reason = obj
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                rule_adjustments.push(RuleWeightAdjustment {
                    rule_id,
                    new_weight,
                    new_decay_rate,
                    reason,
                });
            }
        }

        // 完整规则集替换
        let new_rule_set: Option<EventRuleSet> = data
            .get("new_rule_set")
            .or_else(|| data.get("rule_set"))
            .and_then(|v| serde_json::from_value::<EventRuleSet>(v.clone()).ok())
            .filter(|rs| !rs.rules.is_empty());

        ReflectionOutput {
            baseline_updates,
            affective_baseline_shift: affective_shift,
            rule_adjustments,
            new_rule_set,
            summary: data
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            confidence: data
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_direct() {
        let input = r#"{"baseline_updates": {}, "confidence": 0.5}"#;
        let result = DriveReflection::extract_json(input);
        assert!(result.is_object());
        assert_eq!(result.get("confidence").unwrap().as_f64().unwrap(), 0.5);
    }

    #[test]
    fn test_extract_json_fenced() {
        let input = "```json\n{\"confidence\": 0.7}\n```";
        let result = DriveReflection::extract_json(input);
        assert!(result.is_object());
        assert_eq!(result.get("confidence").unwrap().as_f64().unwrap(), 0.7);
    }

    #[test]
    fn test_extract_json_empty() {
        let result = DriveReflection::extract_json("");
        assert!(result.is_null());
    }

    #[test]
    fn test_parse_reflection_output() {
        let reflection = DriveReflection::new(None, None, 0.3);
        let data = serde_json::json!({
            "baseline_updates": {"social_drive": 0.6},
            "affective_baseline_shift": {"valence": 0.02, "arousal": 0.0, "dominance": 0.01},
            "rule_adjustments": [{"rule_id": "negative_feedback", "new_weight": 1.1, "reason": "test"}],
            "summary": "test summary",
            "confidence": 0.7
        });
        let output = reflection.parse_reflection_output(&data);
        assert_eq!(
            output.baseline_updates.get("social_drive").copied().unwrap(),
            0.6
        );
        assert!((output.confidence - 0.7).abs() < 0.001);
        assert_eq!(output.rule_adjustments.len(), 1);
        assert_eq!(output.summary, "test summary");
    }
}
