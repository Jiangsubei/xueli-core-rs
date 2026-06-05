/// 模糊回忆渲染器 — 用不确定的表达方式包装精确回忆上下文。
///
/// 对应 Python 版 `xueli/src/memory/retrieval/recall_renderer.py`
use chrono::{DateTime, Utc};
use rand::Rng;

/// 模糊回忆渲染器：控制回忆的表达方式。
pub struct RecallRenderer {
    pub enabled: bool,
    pub fuzzy_probability: f64,
    pub confidence_threshold: f64,
    pub confidence_decay_per_day: f64,
    pub confidence_minimum: f64,
}

impl RecallRenderer {
    /// 根据创建时间计算回忆置信度（按天数衰减）
    pub fn compute_confidence(&self, created_at: &str) -> f64 {
        if created_at.is_empty() {
            return 1.0;
        }

        let created_dt = match DateTime::parse_from_rfc3339(created_at) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => {
                // 尝试解析 ISO8601 格式
                match chrono::NaiveDateTime::parse_from_str(created_at, "%Y-%m-%dT%H:%M:%S") {
                    Ok(naive) => naive.and_utc(),
                    Err(_) => return 1.0,
                }
            }
        };

        let now = Utc::now();
        let days = (now - created_dt).num_seconds() as f64 / 86400.0;
        let confidence = 1.0 - days * self.confidence_decay_per_day;
        confidence.max(self.confidence_minimum)
    }

    /// 判断是否应模糊化表达
    pub fn should_fuzzify(&self, confidence: f64) -> bool {
        if !self.enabled {
            return false;
        }
        if confidence >= self.confidence_threshold {
            return false;
        }
        let mut rng = rand::thread_rng();
        rng.gen::<f64>() < self.fuzzy_probability
    }

    /// 返回通用模糊回忆指导文本
    pub fn render_fuzzy_instruction(&self) -> String {
        if !self.enabled {
            return String::new();
        }
        "【回忆提示】当你在回复中引用记忆内容时，请使用自然、模糊的口吻，\
         让回忆听起来像是你脑海中自然浮现的片段，而非精确检索出的内容。\
         不要逐字复述细节，适当加入你自己的想法和感受，\
         用你自己的方式表达不确定感，不需要使用任何预设的开场白。"
            .to_string()
    }

    /// 包装回忆上下文（在原文前添加模糊指导）
    pub fn wrap_recall_context(&self, raw_text: &str) -> String {
        if !self.enabled {
            return raw_text.to_string();
        }
        let instruction = self.render_fuzzy_instruction();
        if instruction.is_empty() {
            return raw_text.to_string();
        }
        format!("{}\n\n{}", instruction, raw_text)
    }

    /// 应用模糊回忆渲染（主入口）
    pub fn apply(&self, recall_context: &str) -> String {
        let text = recall_context.trim();
        if text.is_empty() {
            return String::new();
        }
        if !self.enabled {
            return text.to_string();
        }
        self.wrap_recall_context(text)
    }
}

impl Default for RecallRenderer {
    fn default() -> Self {
        Self {
            enabled: false,
            fuzzy_probability: 0.3,
            confidence_threshold: 0.7,
            confidence_decay_per_day: 0.01,
            confidence_minimum: 0.3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_confidence_empty() {
        let renderer = RecallRenderer::default();
        assert_eq!(renderer.compute_confidence(""), 1.0);
    }

    #[test]
    fn test_compute_confidence_recent() {
        let renderer = RecallRenderer::default();
        let recent = Utc::now().to_rfc3339();
        let confidence = renderer.compute_confidence(&recent);
        assert!(confidence > 0.99);
    }

    #[test]
    fn test_compute_confidence_old() {
        let renderer = RecallRenderer::default();
        let old = "2020-01-01T00:00:00Z";
        let confidence = renderer.compute_confidence(old);
        // 置信度不应低于最小值
        assert!(confidence >= renderer.confidence_minimum);
    }

    #[test]
    fn test_should_fuzzify_disabled() {
        let renderer = RecallRenderer {
            enabled: false,
            ..Default::default()
        };
        assert!(!renderer.should_fuzzify(0.5));
    }

    #[test]
    fn test_should_fuzzify_high_confidence() {
        let renderer = RecallRenderer {
            enabled: true,
            ..Default::default()
        };
        assert!(!renderer.should_fuzzify(0.9));
    }

    #[test]
    fn test_apply_empty() {
        let renderer = RecallRenderer::default();
        assert_eq!(renderer.apply(""), "");
        assert_eq!(renderer.apply("  "), "");
    }

    #[test]
    fn test_apply_with_text() {
        let renderer = RecallRenderer {
            enabled: true,
            ..Default::default()
        };
        let result = renderer.apply("用户喜欢咖啡");
        assert!(result.contains("回忆提示"));
        assert!(result.contains("用户喜欢咖啡"));
    }
}
