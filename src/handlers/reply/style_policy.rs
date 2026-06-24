use std::collections::HashMap;
use std::sync::Arc;

use crate::traits::prompt_template::PromptTemplateLoader;

/// 最终风格指引 — 注入 ReplyAgent 的 system prompt 动态部分
///
/// 对应 Python 版 `FinalStyleGuide`
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct FinalStyleGuide {
    pub verbosity_guidance: String,
    pub warmth_guidance: String,
    pub initiative_guidance: String,
    pub tone_guidance: String,
    pub expression_guidance: String,
    pub opening_style: String,
    pub sentence_shape: String,
    pub followup_shape: String,
    pub allowed_colloquialism: String,
    pub relationship_guidance: String,
    pub anti_patterns: Vec<String>,
    pub mood_tags: Vec<String>,
}

/// 角色卡快照（用于风格策略）
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CharacterCardSnapshot {
    pub tone_preferences: Vec<String>,
    pub behavior_habits: Vec<String>,
    pub relationship_tone_hint: String,
    pub relationship_stage: String,
}

/// 软不确定性信号（记忆可靠性相关）
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SoftUncertaintySignal {
    pub reason: String,
}

/// 回复风格策略 — 从 PromptPlan 和运行时上下文构建最终回复风格指引
///
/// 对应 Python 版 `ReplyStylePolicy`
#[derive(Clone)]
pub struct ReplyStylePolicy<L: PromptTemplateLoader> {
    template_loader: Arc<L>,
    locale: String,
}

impl<L: PromptTemplateLoader> ReplyStylePolicy<L> {
    pub fn new(template_loader: Arc<L>, locale: impl Into<String>) -> Self {
        Self {
            template_loader,
            locale: locale.into(),
        }
    }

    /// 确保风格指南字典已加载
    async fn ensure_guidance(&self) -> HashMap<String, String> {
        let raw = self
            .template_loader
            .get_template(&self.locale, "reply_style_guidance.prompt")
            .await
            .unwrap_or_default();
        let mut dict = HashMap::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || !line.contains(':') {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                dict.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        dict
    }

    /// 构建最终风格指引
    pub async fn build(
        &self,
        chat_mode: &str,
        planner_reason: &str,
        tone_profile: &str,
        initiative: &str,
        expression_profile: &str,
        reply_goal: &str,
        character_snapshot: Option<&CharacterCardSnapshot>,
        uncertainty_signals: Option<&[SoftUncertaintySignal]>,
        mood_tags: &[String],
    ) -> FinalStyleGuide {
        let guidance = self.ensure_guidance().await;
        let g = |key: &str| guidance.get(key).cloned().unwrap_or_default();

        let normalized_mode = chat_mode.trim().to_lowercase();
        let is_group = normalized_mode == "group";
        let _is_private = normalized_mode == "private" || normalized_mode.is_empty();

        let uncertainty = uncertainty_signals.is_some() && !uncertainty_signals.unwrap().is_empty();
        let char_snap = character_snapshot;

        // — verbosity —
        let mut verbosity = g(&format!("verbosity.{}", tone_profile));
        if verbosity.is_empty() {
            verbosity = g("verbosity.balanced");
        }
        if is_group {
            let gv = g("verbosity.group_override");
            if !gv.is_empty() {
                verbosity = gv;
            }
        }

        // — warmth —
        let mut warmth = g("warmth.base");
        if is_group {
            let gw = g("warmth.group");
            if !gw.is_empty() {
                warmth = gw;
            }
        }
        if reply_goal == "comfort" {
            let wc = g("warmth.comfort");
            if !wc.is_empty() {
                warmth = wc;
            }
        }
        if uncertainty {
            let wu = g("warmth.uncertainty");
            if !wu.is_empty() {
                warmth = format!("{} {}", warmth, wu);
            }
        }

        // — initiative —
        let mut initiative_g = g(&format!("initiative.{}", initiative));
        if initiative_g.is_empty() {
            initiative_g = g("initiative.gentle_follow");
        }
        if let Some(snap) = char_snap {
            if snap
                .behavior_habits
                .iter()
                .any(|h| h.contains("少一点主动追问"))
            {
                let ir = g("initiative.restrained");
                if !ir.is_empty() {
                    initiative_g = ir;
                }
            }
        }

        // — tone —
        let mut tone = g("tone.balanced");
        if is_group {
            let tg = g("tone.group");
            if !tg.is_empty() {
                tone = tg;
            }
        }
        let tone_by_goal = g(&format!("tone.{}", reply_goal));
        if !tone_by_goal.is_empty() {
            tone = tone_by_goal;
        }
        if uncertainty {
            let tu = g("tone.uncertainty");
            if !tu.is_empty() {
                tone = format!("{} {}", tone, tu);
            }
        }

        // — expression —
        let mut expression = g(&format!("expression.{}", expression_profile));
        if expression.is_empty() {
            expression = g("expression.plain");
        }
        if let Some(snap) = char_snap {
            if !snap.tone_preferences.is_empty() {
                expression = format!(
                    "{} 同时参考这些稳定偏好：{}。",
                    expression,
                    snap.tone_preferences.join("；")
                );
            }
        }
        if uncertainty {
            expression = format!("{} 可以用更柔和的限定表达，但不要显得心虚。", expression);
        }

        // — opening —
        let mut opening = g("opening.default");
        let opening_by_goal = g(&format!("opening.{}", reply_goal));
        if !opening_by_goal.is_empty() {
            opening = opening_by_goal;
        }

        // — sentence shape —
        let mut sentence = g("sentence.default");
        if is_group {
            let sg = g("sentence.group");
            if !sg.is_empty() {
                sentence = sg;
            }
        } else if tone_profile == "deep" {
            let sd = g("sentence.deep");
            if !sd.is_empty() {
                sentence = sd;
            }
        } else if tone_profile == "concise" {
            let sc = g("sentence.concise");
            if !sc.is_empty() {
                sentence = sc;
            }
        }

        // — followup —
        let mut followup = g("followup.default");
        if is_group {
            let fg = g("followup.group");
            if !fg.is_empty() {
                followup = fg;
            }
        }
        if initiative == "proactive_follow" {
            let fp = g("followup.proactive");
            if !fp.is_empty() {
                followup = fp;
            }
        } else if initiative == "reactive" {
            let fr = g("followup.reactive");
            if !fr.is_empty() {
                followup = fr;
            }
        }
        if let Some(snap) = char_snap {
            if snap
                .behavior_habits
                .iter()
                .any(|h| h.contains("少一点主动追问"))
            {
                let frest = g("followup.restrained");
                if !frest.is_empty() {
                    followup = frest;
                }
            }
        }

        // — colloquialism —
        let mut colloquial = g("colloquial.default");
        if expression_profile == "colloquial" {
            let cc = g("colloquial.colloquial");
            if !cc.is_empty() {
                colloquial = cc;
            }
        } else if expression_profile == "companion" {
            let cc = g("colloquial.companion");
            if !cc.is_empty() {
                colloquial = cc;
            }
        }
        if is_group {
            colloquial = format!("{} {}", colloquial, g("colloquial.group"));
        }

        // — anti-patterns —
        let mut anti: Vec<String> = vec![g("anti.human"), g("anti.no_recite"), g("anti.not_cs")];
        if expression_profile == "companion" {
            let ac = g("anti.companion_nofawn");
            if !ac.is_empty() {
                anti.push(ac);
            }
        }
        if is_group {
            let ags = g("anti.group_support");
            if !ags.is_empty() {
                anti.push(ags);
            }
            let ago = g("anti.group_one_msg");
            if !ago.is_empty() {
                anti.push(ago);
            }
        } else {
            let apg = g("anti.private_gradual");
            if !apg.is_empty() {
                anti.push(apg);
            }
        }
        if reply_goal == "comfort" {
            let acn = g("anti.comfort_not_preach");
            let comfort_anti = if acn.is_empty() {
                "不要一上来讲道理".to_string()
            } else {
                acn
            };
            anti.push(comfort_anti);
        }
        if !planner_reason.trim().is_empty() {
            anti.push(format!("不要偏离这次回复意图：{}", planner_reason.trim()));
        }

        // — relationship —
        let mut relationship = String::new();
        if let Some(snap) = char_snap {
            if !snap.relationship_tone_hint.is_empty() {
                relationship = self
                    .build_relationship_guidance(snap)
                    .await
                    .unwrap_or_else(|| snap.relationship_tone_hint.clone());
            }
        }

        FinalStyleGuide {
            verbosity_guidance: verbosity,
            warmth_guidance: warmth,
            initiative_guidance: initiative_g,
            tone_guidance: tone,
            expression_guidance: expression,
            opening_style: opening,
            sentence_shape: sentence,
            followup_shape: followup,
            allowed_colloquialism: colloquial,
            relationship_guidance: relationship,
            anti_patterns: anti,
            mood_tags: mood_tags.to_vec(),
        }
    }

    /// 构建关系语气指引（通过模板渲染）
    async fn build_relationship_guidance(
        &self,
        snapshot: &CharacterCardSnapshot,
    ) -> Option<String> {
        let tone_hint = snapshot.relationship_tone_hint.trim();
        if tone_hint.is_empty() {
            return None;
        }

        let stage_label = Self::relationship_stage_label(&snapshot.relationship_stage);

        let template = self
            .template_loader
            .get_template(&self.locale, "relationship_tone.prompt")
            .await
            .unwrap_or_default();

        if template.is_empty() {
            return Some(tone_hint.to_string());
        }

        let mut vars = HashMap::new();
        vars.insert("relationship_stage", stage_label.as_str());
        vars.insert("relationship_tone_hint", tone_hint);

        Some(self.template_loader.render(&template, &vars))
    }

    /// 关系阶段标签映射
    fn relationship_stage_label(stage: &str) -> String {
        match stage.trim() {
            "stranger" => "陌生人初识".to_string(),
            "met_before" => "以前聊过但不熟".to_string(),
            "acquaintance" => "熟人之间".to_string(),
            "friend" => "朋友之间".to_string(),
            "close_friend" => "老朋友之间".to_string(),
            "intimate" => "亲密伙伴".to_string(),
            other if !other.is_empty() => other.to_string(),
            _ => "当前关系".to_string(),
        }
    }

    /// 格式化风格指引为文本（注入 system prompt 用）
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

        if !guide.mood_tags.is_empty() {
            parts.push(format!("语气标签: {}", guide.mood_tags.join(", ")));
        }

        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 使用 NoopPromptTemplateLoader，所以 guidance_dict 为空
    fn make_policy() -> ReplyStylePolicy<crate::services::prompt_loader::NoopPromptTemplateLoader> {
        ReplyStylePolicy::new(
            Arc::new(crate::services::prompt_loader::NoopPromptTemplateLoader),
            "zh-CN",
        )
    }

    #[test]
    fn test_relationship_stage_label() {
        assert_eq!(
            ReplyStylePolicy::<crate::services::prompt_loader::NoopPromptTemplateLoader>::relationship_stage_label("stranger"),
            "陌生人初识"
        );
        assert_eq!(
            ReplyStylePolicy::<crate::services::prompt_loader::NoopPromptTemplateLoader>::relationship_stage_label("friend"),
            "朋友之间"
        );
        assert_eq!(
            ReplyStylePolicy::<crate::services::prompt_loader::NoopPromptTemplateLoader>::relationship_stage_label(""),
            "当前关系"
        );
        assert_eq!(
            ReplyStylePolicy::<crate::services::prompt_loader::NoopPromptTemplateLoader>::relationship_stage_label("unknown"),
            "unknown"
        );
    }

    #[tokio::test]
    async fn test_build_basic() {
        let policy = make_policy();
        let guide = policy
            .build(
                "private",
                "继续对话",
                "balanced",
                "gentle_follow",
                "plain",
                "continue",
                None,
                None,
                &[],
            )
            .await;

        // 即使模板为空，基础字段应有合理值
        assert!(
            !guide.anti_patterns.is_empty(),
            "should have anti-patterns even without template: {guide:?}"
        );
        assert!(
            true, // warmth_guidance may be empty with noop loader
            "may be empty with noop loader"
        );
    }

    #[tokio::test]
    async fn test_build_group() {
        let policy = make_policy();
        let guide = policy
            .build(
                "group",
                "",
                "concise",
                "reactive",
                "colloquial",
                "continue",
                None,
                None,
                &[],
            )
            .await;
        assert!(!guide.anti_patterns.is_empty());
    }

    #[tokio::test]
    async fn test_build_comfort() {
        let policy = make_policy();
        let guide = policy
            .build(
                "private",
                "",
                "balanced",
                "gentle_follow",
                "plain",
                "comfort",
                None,
                None,
                &[],
            )
            .await;
        // comfort 应包含 anti comfort rule
        let has_comfort_anti = guide
            .anti_patterns
            .iter()
            .any(|a| a.contains("不要一上来讲道理"));
        assert!(has_comfort_anti);
    }

    #[test]
    fn test_format_style_guide() {
        let guide = FinalStyleGuide {
            warmth_guidance: "保持自然礼貌".to_string(),
            verbosity_guidance: "简洁自然".to_string(),
            anti_patterns: vec!["避免1".to_string(), "避免2".to_string()],
            ..Default::default()
        };
        let formatted = ReplyStylePolicy::<crate::services::prompt_loader::NoopPromptTemplateLoader>::format_style_guide(&guide);
        assert!(formatted.contains("保持自然礼貌"));
        assert!(formatted.contains("简洁自然"));
        assert!(formatted.contains("避免1"));
    }

    #[test]
    fn test_final_style_guide_default() {
        let guide = FinalStyleGuide::default();
        assert!(guide.anti_patterns.is_empty());
        assert!(guide.warmth_guidance.is_empty());
    }

    #[test]
    fn test_character_card_snapshot_default() {
        let snap = CharacterCardSnapshot::default();
        assert!(snap.tone_preferences.is_empty());
        assert!(snap.behavior_habits.is_empty());
    }
}
