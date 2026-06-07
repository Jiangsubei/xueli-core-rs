use std::collections::HashMap;
use std::sync::Arc;

use crate::character::card_service::CharacterCardSnapshot;
use crate::core::scope::ChatScope;
use crate::handlers::reply::style_policy::{FinalStyleGuide, SoftUncertaintySignal};
use crate::prelude::XueliResult;
use crate::signals::label_mapper::{conversation_window_label, mood_decision_label};
use crate::traits::prompt_template::PromptTemplateLoader;

/// ReplyAgent 提示词构建器
///
/// 对应 Python 版 `xueli/src/handlers/reply/prompt_builder.py`
pub struct ReplyPromptBuilder<L: PromptTemplateLoader> {
    template_loader: Arc<L>,
    locale: String,
}

impl<L: PromptTemplateLoader> ReplyPromptBuilder<L> {
    pub fn new(template_loader: Arc<L>, locale: impl Into<String>) -> Self {
        Self {
            template_loader,
            locale: locale.into(),
        }
    }

    /// 构建系统提示词
    pub async fn build_system_prompt(
        &self,
        identity: &str,
        scope: &ChatScope,
        style_guide: Option<&FinalStyleGuide>,
        person_facts: &[String],
        memories: &[String],
        character_card_snapshot: Option<&CharacterCardSnapshot>,
        narrative_thread_summary: Option<&str>,
        narrative_thread_label: Option<&str>,
        narrative_self: Option<&HashMap<String, String>>,
        planning_signals: Option<&HashMap<String, String>>,
        user_emotion_label: Option<&str>,
        soft_uncertainty_signals: Option<&[SoftUncertaintySignal]>,
        caution_signal: Option<&HashMap<String, String>>,
        metacognition_state_report: Option<&str>,
        user_profile_signal: Option<&HashMap<String, String>>,
        reply_reference: Option<&str>,
    ) -> String {
        let stable = self
            .build_stable_system_prompt(identity, scope)
            .await
            .unwrap_or_else(|_| self.fallback_system_prompt(identity, scope));
        let dynamic = self.build_dynamic_context(
            style_guide,
            person_facts,
            memories,
            character_card_snapshot,
            narrative_thread_summary,
            narrative_thread_label,
            narrative_self,
            planning_signals,
            user_emotion_label,
            soft_uncertainty_signals,
            caution_signal,
            metacognition_state_report,
            user_profile_signal,
            reply_reference,
        );

        if dynamic.is_empty() {
            stable
        } else {
            format!("{}\n\n{}", stable, dynamic)
        }
    }

    /// 构建稳定部分（模板文件）
    async fn build_stable_system_prompt(
        &self,
        identity: &str,
        scope: &ChatScope,
    ) -> XueliResult<String> {
        let session_label = if scope.is_group() { "群聊" } else { "私聊" };

        let scene_file = if scope.is_group() {
            "scene_guidance_group.prompt"
        } else {
            "scene_guidance_private.prompt"
        };

        let scene_guidance = self
            .template_loader
            .get_template(&self.locale, scene_file)
            .await
            .unwrap_or_default();

        let base = self
            .template_loader
            .get_template(&self.locale, "reply_agent_system_base.prompt")
            .await
            .unwrap_or_else(|_| String::new());

        let rendered = if base.is_empty() {
            self.fallback_system_prompt(identity, scope)
        } else {
            let mut vars = std::collections::HashMap::new();
            vars.insert("identity", identity);
            vars.insert("session_label", session_label);
            vars.insert("scene_guidance", &scene_guidance);
            self.template_loader.render(&base, &vars)
        };

        Ok(rendered)
    }

    /// 构建动态上下文 — 注入 15+ 上下文块
    ///
    /// 对应 Python 版 `build_dynamic_context()`
    fn build_dynamic_context(
        &self,
        style_guide: Option<&FinalStyleGuide>,
        person_facts: &[String],
        memories: &[String],
        character_card_snapshot: Option<&CharacterCardSnapshot>,
        narrative_thread_summary: Option<&str>,
        narrative_thread_label: Option<&str>,
        narrative_self: Option<&HashMap<String, String>>,
        planning_signals: Option<&HashMap<String, String>>,
        user_emotion_label: Option<&str>,
        soft_uncertainty_signals: Option<&[SoftUncertaintySignal]>,
        caution_signal: Option<&HashMap<String, String>>,
        metacognition_state_report: Option<&str>,
        user_profile_signal: Option<&HashMap<String, String>>,
        reply_reference: Option<&str>,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();

        // 1. 回复风格指引
        if let Some(guide) = style_guide {
            let formatted = Self::format_style_guide(guide);
            if !formatted.is_empty() {
                parts.push(format!("【回复风格指引】\n{}", formatted));
            }
        }

        // 2. 用户已知信息 / 人物事实
        if !person_facts.is_empty() {
            parts.push(format!("【用户已知信息】\n{}", person_facts.join("\n")));
        }

        // 3. 长期记忆
        if !memories.is_empty() {
            parts.push(format!("【长期记忆】\n{}", memories.join("\n")));
        }

        // 4. 角色卡快照 → 关系状态
        if let Some(snap) = character_card_snapshot {
            let mut rel_parts: Vec<String> = Vec::new();

            let persona_hints: String = snap
                .bot_persona_hints
                .iter()
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("；");
            if !persona_hints.is_empty() {
                rel_parts.push(format!("回复指引={}", persona_hints));
            }

            if !snap.emotional_trend.is_empty() {
                rel_parts.push(format!("近期情绪趋势={}", snap.emotional_trend));
            }

            if !snap.relationship_stage.is_empty() && snap.relationship_stage != "stranger" {
                let stage_label = match snap.relationship_stage.as_str() {
                    "met_before" => "以前聊过但不熟",
                    "acquaintance" => "熟人之间",
                    "friend" => "朋友之间",
                    "close_friend" => "老朋友之间",
                    "intimate" => "亲密伙伴",
                    other => other,
                };
                rel_parts.push(format!("关系阶段={}", stage_label));
            }

            if !snap.tone_preferences.is_empty() {
                let prefs = snap.tone_preferences.join("；");
                if !prefs.is_empty() {
                    rel_parts.push(format!("长期风格适应={}", prefs));
                }
            }

            if !rel_parts.is_empty() {
                parts.push(format!("【关系状态】\n{}", rel_parts.join("\n")));
            }
        }

        // 5. 对话主线 (narrative thread)
        if let Some(summary) = narrative_thread_summary {
            let summary = summary.trim();
            if !summary.is_empty() {
                if let Some(label) = narrative_thread_label {
                    let label = label.trim();
                    if !label.is_empty() {
                        parts.push(format!("【对话主线】\n{}：{}", label, summary));
                    } else {
                        parts.push(format!("【对话主线】\n{}", summary));
                    }
                } else {
                    parts.push(format!("【对话主线】\n{}", summary));
                }
            }
        }

        // 6. 长期相处脉络 (narrative_self)
        if let Some(ns) = narrative_self {
            let mut self_parts: Vec<String> = Vec::new();

            if let Some(story) = ns.get("relationship_story") {
                let story = story.trim();
                if !story.is_empty() {
                    self_parts.push(story.to_string());
                }
            }

            if self_parts.is_empty() {
                let events: Vec<&str> = ns
                    .iter()
                    .filter(|(k, v)| k.starts_with("event") && !v.trim().is_empty())
                    .map(|(_, v)| v.as_str())
                    .collect();
                if !events.is_empty() {
                    self_parts.push(events.join("\n"));
                }
            }

            if !self_parts.is_empty() {
                parts.push(format!("【长期相处脉络】\n{}", self_parts.join("\n")));
            }
        }

        // 7. 用户画像 (user_profile_signal)
        if let Some(profile) = user_profile_signal {
            let mut profile_parts: Vec<String> = Vec::new();

            for key in &[
                "style_summary",
                "preferred_response_shape",
                "followup_preference",
                "directness",
                "relationship_read",
                "emotional_safety",
                "tone_guidance",
                "last_emotional_tone",
            ] {
                if let Some(val) = profile.get(*key) {
                    let val = val.trim();
                    if !val.is_empty() {
                        match *key {
                            "preferred_response_shape" => {
                                profile_parts.push(format!("回复形态={}", val));
                            }
                            "followup_preference" => {
                                profile_parts.push(format!("追问偏好={}", val));
                            }
                            "directness" => {
                                profile_parts.push(format!("直接程度={}", val));
                            }
                            "emotional_safety" => {
                                profile_parts.push(format!("情绪安全感={}", val));
                            }
                            "last_emotional_tone" => {
                                profile_parts.push(format!("近期情绪={}", val));
                            }
                            _ => {
                                profile_parts.push(val.to_string());
                            }
                        }
                    }
                }
            }

            if !profile_parts.is_empty() {
                parts.push(format!("【用户画像】\n{}", profile_parts.join("\n")));
            }
        }

        // 8. 情绪感知 (mood_state / planner mood)
        if let Some(profile) = user_profile_signal {
            if let Some(mood) = profile.get("mood_state") {
                let mood = mood.trim();
                if !mood.is_empty() {
                    parts.push(format!("【情绪感知】\n{}", mood));
                }
            }
        }

        // 9. 参与倾向 (mood_decision)
        if let Some(signals) = planning_signals {
            if let Some(md_val) = signals.get("mood_decision") {
                let md_str = md_val.trim();
                if !md_str.is_empty() {
                    // mood_decision 可能是 JSON 字符串或已展开的 key=value 对
                    // 尝试解析为 HashMap<String,String> 并用 label mapper
                    if let Ok(md_map) = serde_json::from_str::<HashMap<String, String>>(md_str) {
                        let label = mood_decision_label(&md_map);
                        if !label.is_empty() {
                            parts.push(format!("【参与倾向】\n{}", label));
                        }
                    } else {
                        parts.push(format!("【参与倾向】\n{}", md_str));
                    }
                }
            }
        }

        // 10. 群聊窗口 (conversation_window)
        if let Some(signals) = planning_signals {
            if let Some(cw_val) = signals.get("conversation_window") {
                let cw_str = cw_val.trim();
                if !cw_str.is_empty() {
                    if let Ok(cw_map) = serde_json::from_str::<HashMap<String, String>>(cw_str) {
                        let label = conversation_window_label(&cw_map);
                        if !label.is_empty() {
                            parts.push(format!("【群聊窗口】\n{}", label));
                        }
                    } else {
                        parts.push(format!("【群聊窗口】\n{}", cw_str));
                    }
                }
            }
        }

        // 11. 当前用户情绪
        if let Some(label) = user_emotion_label {
            let label = label.trim();
            if !label.is_empty() {
                parts.push(format!("【当前用户情绪】\n{}", label));
            }
        }

        // 12. 回复参考
        if let Some(reference) = reply_reference {
            let reference = reference.trim();
            if !reference.is_empty() {
                parts.push(format!("【回复参考】\n{}", reference));
            }
        }

        // 13. 记忆可靠性 (soft_uncertainty_signals)
        if let Some(signals) = soft_uncertainty_signals {
            if !signals.is_empty() {
                parts.push("【记忆可靠性】\n部分记忆存在不确定性，回复时适当保留余地".to_string());
            }
        }

        // 14. 谨慎度 (caution_signal)
        if let Some(caution) = caution_signal {
            let level = caution
                .get("caution_level")
                .map(|s| s.as_str())
                .unwrap_or("");
            if !level.is_empty() && level != "low" {
                let mut lines: Vec<String> = Vec::new();
                lines.push(format!("级别={}", level));

                if let Some(reasons) = caution.get("caution_reasons") {
                    let reasons = reasons.trim();
                    if !reasons.is_empty() {
                        lines.push(format!("原因={}", reasons));
                    }
                }

                if let Some(guidance) = caution.get("reply_guidance") {
                    let guidance = guidance.trim();
                    if !guidance.is_empty() {
                        lines.push(format!("回复要求={}", guidance));
                    }
                }

                parts.push(format!("【谨慎度】\n{}", lines.join("\n")));
            }
        }

        // 15. 自我状态 (metacognition_state_report)
        if let Some(report) = metacognition_state_report {
            let report = report.trim();
            if !report.is_empty() {
                parts.push(format!("【自我状态】\n{}", report));
            }
        }

        parts.join("\n\n")
    }

    /// 兜底提示词（模板加载失败时使用）
    fn fallback_system_prompt(&self, identity: &str, scope: &ChatScope) -> String {
        let session_label = if scope.is_group() { "群聊" } else { "私聊" };
        let scene = if scope.is_group() {
            "群聊场景：保持轻量参与，只在被点名或话题自然轮到你时回应。回复要短（一句到两句），不要长篇大论。"
        } else {
            "私聊场景：自然亲切地回复，但要围绕当前消息，不要假装一直在连续对话。"
        };
        format!(
            "{}\n\n当前会话类型：{session_label}。\n\n\
            你拥有以下工具：\n\
            - reply(text, segments?) — 发送回复文本。调用后本轮结束\n\
            - query_memory(query) — 查询关于当前用户的相关记忆\n\
            - query_person(name) — 查询用户档案\n\
            - view_message(msg_id) — 查看指定消息的完整内容\n\
            - tool_search(query) — 搜索可用工具\n\
            调用 reply 后本轮结束。\n\n\
            {scene}",
            identity
        )
    }

    /// 格式化风格指引 — 对应 Python 版 `_format_style_guide()`
    ///
    /// 从 FinalStyleGuide 中提取所有非空字段格式化为注入文本。
    pub fn format_style_guide(guide: &FinalStyleGuide) -> String {
        let mut parts: Vec<String> = Vec::new();

        for val in [
            &guide.warmth_guidance,
            &guide.verbosity_guidance,
            &guide.initiative_guidance,
            &guide.tone_guidance,
            &guide.expression_guidance,
            &guide.opening_style,
            &guide.sentence_shape,
            &guide.followup_shape,
        ] {
            if !val.is_empty() {
                parts.push(format!("- {}", val));
            }
        }

        if !guide.anti_patterns.is_empty() {
            parts.push("避免以下模式：".to_string());
            for a in &guide.anti_patterns {
                parts.push(format!("  - {}", a));
            }
        }

        parts.join("\n")
    }
}
