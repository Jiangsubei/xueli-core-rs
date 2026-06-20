use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

use crate::core::platform_types::InboundEvent;
use crate::handlers::planner::{MemoryProfile, PromptPlan, PromptSectionPolicy, SectionIntensity};

static PROMPT_NOTES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn prompt_notes_map() -> &'static Mutex<HashMap<String, String>> {
    PROMPT_NOTES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 提示词规划器 — 构建提示策略的默认值，并解析 LLM 返回的规划决策。
///
/// 负责：生成决策输出 schema、从 LLM 决策 JSON 解析 PromptPlan、根据场景生成合理的默认计划。
#[derive(Debug, Clone, Default)]
pub struct PromptPlanner;

impl PromptPlanner {
    /// 懒加载 prompt_notes.prompt 并返回指定 key 的提示文本
    fn get_prompt_note(key: &str) -> String {
        {
            let cache = prompt_notes_map().lock().unwrap();
            if let Some(v) = cache.get(key) {
                return v.clone();
            }
        }
        let path = std::path::PathBuf::from("prompts/zh-CN/prompt_notes.prompt");
        if let Ok(raw) = std::fs::read_to_string(&path) {
            let mut map = HashMap::new();
            for line in raw.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    let key_part = k.trim();
                    if key_part.starts_with("note.") {
                        map.insert(key_part.to_string(), v.trim().to_string());
                    }
                }
            }
            let mut cache = prompt_notes_map().lock().unwrap();
            *cache = map;
            return cache.get(key).cloned().unwrap_or_default();
        }
        String::new()
    }

    /// 返回规划器留给 LLM 的决策输出 schema（JSON 格式说明），用于注入到 system prompt
    pub fn decision_output_schema(&self, emoji_enabled: bool) -> String {
        let mut base = String::from(r#"{"reason":"简短理由","#);
        base.push_str(r#""prompt_plan":{"#);
        base.push_str(r#""reply_goal":"answer|continue|comfort|clarify|recall|light_presence","#);
        base.push_str(
            r#""continuity_mode":"direct_continue|resume_recent_topic|resume_old_topic","#,
        );
        base.push_str(r#""timeline_detail":"off|summary|per_message","#);
        base.push_str(r#""context_profile":"compact|standard|full","#);
        base.push_str(r#""memory_profile":"off|facts_only|relevant|rich","#);
        base.push_str(r#""tone_profile":"concise|balanced|warm|deep","#);
        base.push_str(r#""initiative":"reactive|gentle_follow|proactive_follow","#);
        base.push_str(r#""expression_profile":"plain|colloquial|companion","#);
        base.push_str(r#""policy":{"#);
        base.push_str(
            r#""include_recent_history":true,"include_person_facts":true,"include_session_restore":true,"#,
        );
        base.push_str(
            r#""include_precise_recall":true,"include_dynamic_memory":true,"include_vision_context":true,"#,
        );
        base.push_str(r#""include_reply_scope":true,"include_style_guide":true},"#);
        base.push_str(r#""notes":"可选说明"},"#);
        base.push_str(r#""predicted_user_response":"你预测用户下一轮会如何回应，可选","#);
        base.push_str(r#""reply_reference":"给回复模型看的自然语言参考，可选","#);
        base.push_str(r#""narrative_signal":{"narrative_label":"当前话题标签，可选","narrative_summary":"当前对话主线摘要，可选","confidence":0.0,"reason":"可选说明"},"#);
        base.push_str(r#""reply_adaptation_signal":{"style_summary":"即时风格建议，可选","preferred_response_shape":"短句|一两句|分点|解释型等，可选","followup_preference":"是否适合追问，可选","directness":"直接程度，可选","humor_tolerance":"幽默容忍度，可选","relationship_read":"当前关系理解，可选","trust_level":0.0,"formality_distance":0.5,"emotional_safety":"low|medium|high，可选","tone_guidance":"语气建议，可选","last_emotional_tone":"当前/近期情绪口径，可选","mood_state":"以第三人称客观分析助手的真实情绪感受，可选","intimacy_delta":0.0,"confidence":0.0,"reason":"可选说明"},"#);
        base.push_str(
            r#""mood_adjustments":{"valence_delta":0.0,"energy_delta":0.0,"arousal_delta":0.0},"#,
        );
        base.push_str(r#""system_recommendations":{"next_participation_energy":0.5,"preferred_wait_seconds":null,"preferred_cooldown_seconds":null},"#);
        base.push_str(r#""caution_hint":{"risk_posture":"normal|careful，可选","reply_guidance":"谨慎回复建议，可选"}"#);
        if emoji_enabled {
            base.push_str(r#","emoji_should_send":true,"emoji_intent_reference":"自然语言描述适合什么类型的表情，可选""#);
        }
        base.push('}');
        base
    }

    /// 将 LLM 返回的决策 JSON 解析为 PromptPlan，LLM 未提供的字段回退为默认值
    pub fn parse_prompt_plan(
        &self,
        decision: Option<&serde_json::Map<String, serde_json::Value>>,
        event: &InboundEvent,
        is_group: bool,
        is_first_turn: bool,
    ) -> Option<PromptPlan> {
        let decision = decision?;
        let action = decision
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("reply");
        if action != "reply" {
            return None;
        }

        let is_group = event
            .message
            .as_ref()
            .map(|m| m.scope.is_group())
            .unwrap_or(is_group);
        let default_plan = self.default_prompt_plan(is_group, "unknown", is_first_turn, false);

        let raw_plan = match decision.get("prompt_plan").and_then(|v| v.as_object()) {
            Some(rp) => rp,
            None => return Some(default_plan),
        };

        let raw_policy = raw_plan.get("policy").and_then(|v| v.as_object());
        let policy = PromptSectionPolicy {
            include_recent_history: raw_policy
                .and_then(|p| p.get("include_recent_history"))
                .and_then(|v| v.as_bool())
                .unwrap_or(default_plan.policy.include_recent_history),
            include_person_facts: raw_policy
                .and_then(|p| p.get("include_person_facts"))
                .and_then(|v| v.as_bool())
                .unwrap_or(default_plan.policy.include_person_facts),
            include_session_restore: raw_policy
                .and_then(|p| p.get("include_session_restore"))
                .and_then(|v| v.as_bool())
                .unwrap_or(default_plan.policy.include_session_restore),
            include_precise_recall: raw_policy
                .and_then(|p| p.get("include_precise_recall"))
                .and_then(|v| v.as_bool())
                .unwrap_or(default_plan.policy.include_precise_recall),
            include_dynamic_memory: raw_policy
                .and_then(|p| p.get("include_dynamic_memory"))
                .and_then(|v| v.as_bool())
                .unwrap_or(default_plan.policy.include_dynamic_memory),
            include_vision_context: raw_policy
                .and_then(|p| p.get("include_vision_context"))
                .and_then(|v| v.as_bool())
                .unwrap_or(default_plan.policy.include_vision_context),
            include_reply_scope: raw_policy
                .and_then(|p| p.get("include_reply_scope"))
                .and_then(|v| v.as_bool())
                .unwrap_or(default_plan.policy.include_reply_scope),
            include_style_guide: raw_policy
                .and_then(|p| p.get("include_style_guide"))
                .and_then(|v| v.as_bool())
                .unwrap_or(default_plan.policy.include_style_guide),
        };

        Some(PromptPlan {
            reply_goal: Self::normalize_choice(
                raw_plan.get("reply_goal").and_then(|v| v.as_str()),
                &[
                    "answer",
                    "continue",
                    "comfort",
                    "clarify",
                    "recall",
                    "light_presence",
                ],
                &default_plan.reply_goal,
            ),
            continuity_mode: Self::normalize_choice(
                raw_plan.get("continuity_mode").and_then(|v| v.as_str()),
                &["direct_continue", "resume_recent_topic", "resume_old_topic"],
                &default_plan.continuity_mode,
            ),
            timeline_detail: Self::normalize_choice(
                raw_plan.get("timeline_detail").and_then(|v| v.as_str()),
                &["off", "summary", "per_message"],
                &default_plan.timeline_detail,
            ),
            context_profile: Self::normalize_choice(
                raw_plan.get("context_profile").and_then(|v| v.as_str()),
                &["compact", "standard", "full"],
                &default_plan.context_profile,
            ),
            memory_profile: Self::parse_memory_profile(
                raw_plan.get("memory_profile").and_then(|v| v.as_str()),
                default_plan.memory_profile,
            ),
            tone_profile: Self::normalize_choice(
                raw_plan.get("tone_profile").and_then(|v| v.as_str()),
                &["concise", "balanced", "warm", "deep"],
                &default_plan.tone_profile,
            ),
            initiative: Self::normalize_choice(
                raw_plan.get("initiative").and_then(|v| v.as_str()),
                &["reactive", "gentle_follow", "proactive_follow"],
                &default_plan.initiative,
            ),
            expression_profile: Self::normalize_choice(
                raw_plan.get("expression_profile").and_then(|v| v.as_str()),
                &["plain", "colloquial", "companion"],
                &default_plan.expression_profile,
            ),
            personality_mode: Self::normalize_choice(
                raw_plan.get("personality_mode").and_then(|v| v.as_str()),
                &["balanced", "playful", "serious", "warm", "reserved"],
                &default_plan.personality_mode,
            ),
            conversation_style: Self::normalize_choice(
                raw_plan.get("conversation_style").and_then(|v| v.as_str()),
                &["standard", "casual", "formal", "intimate", "distant"],
                &default_plan.conversation_style,
            ),
            mood_instruction: raw_plan
                .get("mood_instruction")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or(default_plan.mood_instruction),
            planner_reminder: raw_plan
                .get("planner_reminder")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or(default_plan.planner_reminder),
            emoji_instruction: raw_plan
                .get("emoji_instruction")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or(default_plan.emoji_instruction),
            policy,
            notes: raw_plan
                .get("notes")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or(default_plan.notes),
            section_intensity: Self::parse_section_intensity(
                raw_plan
                    .get("section_intensity")
                    .and_then(|v| v.as_object()),
            ),
            emoji_should_send: decision
                .get("emoji_should_send")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            emoji_intent_reference: decision
                .get("emoji_intent_reference")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default(),
        })
    }

    /// 在不调用 LLM 的情况下，根据场景（群聊/私聊、连续性、首轮等）生成默认提示策略
    pub fn default_prompt_plan(
        &self,
        is_group: bool,
        continuity_hint: &str,
        is_first_turn: bool,
        follows_assistant_recently: bool,
    ) -> PromptPlan {
        let chat_mode = if is_group { "group" } else { "private" };

        let reply_goal =
            Self::default_reply_goal(chat_mode, continuity_hint, follows_assistant_recently);

        let continuity_mode = match continuity_hint {
            "strong_continuation" => "direct_continue",
            "soft_continuation" => "resume_recent_topic",
            "resume_after_break" => "resume_recent_topic",
            "old_topic_resume" => "resume_old_topic",
            _ => {
                if !is_group {
                    "resume_recent_topic"
                } else {
                    "direct_continue"
                }
            }
        };

        let mut continuity_mode_final = continuity_mode;
        if !is_group
            && is_first_turn
            && (continuity_hint == "unknown" || continuity_hint == "strong_continuation")
            && !follows_assistant_recently
        {
            continuity_mode_final = "resume_recent_topic";
        }

        let timeline_detail = match continuity_hint {
            "old_topic_resume" => "per_message",
            "unknown" => "off",
            _ => "per_message",
        };

        let context_profile = match continuity_hint {
            "resume_after_break" | "old_topic_resume" => "full",
            _ => {
                if is_group {
                    "compact"
                } else {
                    "standard"
                }
            }
        };

        let memory_profile = if reply_goal == "clarify" && is_group {
            MemoryProfile::Off
        } else if reply_goal == "answer" && continuity_hint == "old_topic_resume" {
            MemoryProfile::Rich
        } else if reply_goal == "comfort" {
            if is_group {
                MemoryProfile::FactsOnly
            } else {
                MemoryProfile::Relevant
            }
        } else if continuity_hint == "old_topic_resume" {
            MemoryProfile::Rich
        } else {
            MemoryProfile::Relevant
        };

        let tone_profile = if reply_goal == "comfort" {
            "warm"
        } else if continuity_hint == "old_topic_resume" {
            "deep"
        } else if reply_goal == "clarify" && is_group {
            "concise"
        } else if is_group {
            "concise"
        } else {
            "balanced"
        };

        let initiative = if is_group {
            "reactive"
        } else {
            "gentle_follow"
        };
        let initiative = if matches!(reply_goal, "continue" | "recall") {
            "gentle_follow"
        } else {
            initiative
        };

        let expression_profile = if reply_goal == "comfort" {
            "companion"
        } else if matches!(reply_goal, "continue" | "light_presence") {
            "colloquial"
        } else {
            "plain"
        };

        let should_reply = true;
        let include_person_facts = should_reply
            && !is_group
            && matches!(
                memory_profile,
                MemoryProfile::FactsOnly | MemoryProfile::Relevant | MemoryProfile::Rich
            );
        let include_session_restore =
            should_reply && matches!(continuity_hint, "resume_after_break" | "old_topic_resume");
        let include_precise_recall = should_reply && continuity_hint == "old_topic_resume";
        let include_dynamic_memory = should_reply
            && matches!(
                memory_profile,
                MemoryProfile::Relevant | MemoryProfile::Rich
            );

        let policy = PromptSectionPolicy {
            include_recent_history: should_reply,
            include_person_facts,
            include_session_restore,
            include_precise_recall,
            include_dynamic_memory,
            include_vision_context: should_reply,
            include_reply_scope: should_reply,
            include_style_guide: should_reply,
        };

        let notes = Self::default_notes(reply_goal, continuity_hint, is_group);

        PromptPlan {
            reply_goal: reply_goal.to_string(),
            continuity_mode: continuity_mode_final.to_string(),
            timeline_detail: timeline_detail.to_string(),
            context_profile: context_profile.to_string(),
            memory_profile,
            tone_profile: tone_profile.to_string(),
            initiative: initiative.to_string(),
            expression_profile: expression_profile.to_string(),
            personality_mode: "balanced".to_string(),
            conversation_style: "standard".to_string(),
            mood_instruction: String::new(),
            planner_reminder: String::new(),
            emoji_instruction: String::new(),
            policy,
            notes,
            emoji_should_send: false,
            emoji_intent_reference: String::new(),
            section_intensity: HashMap::new(),
        }
    }

    fn default_reply_goal(
        chat_mode: &str,
        continuity_hint: &str,
        follows_assistant_recently: bool,
    ) -> &'static str {
        if continuity_hint == "old_topic_resume" {
            return "recall";
        }
        if follows_assistant_recently {
            return "continue";
        }
        if chat_mode == "group" {
            "light_presence"
        } else {
            "answer"
        }
    }

    fn default_notes(reply_goal: &str, continuity_hint: &str, is_group: bool) -> String {
        let mut notes: Vec<String> = Vec::new();
        if reply_goal == "comfort" {
            if let Some(n) = Self::get_prompt_note_nz("note.comfort") {
                notes.push(n);
            }
        }
        if reply_goal == "clarify" {
            if let Some(n) = Self::get_prompt_note_nz("note.clarify") {
                notes.push(n);
            }
        }
        if reply_goal == "light_presence" {
            if let Some(n) = Self::get_prompt_note_nz("note.light_presence") {
                notes.push(n);
            }
        }
        if continuity_hint == "old_topic_resume" {
            if let Some(n) = Self::get_prompt_note_nz("note.old_topic_resume") {
                notes.push(n);
            }
        }
        if is_group {
            if let Some(n) = Self::get_prompt_note_nz("note.group") {
                notes.push(n);
            }
        }
        notes.join(" ")
    }

    fn get_prompt_note_nz(key: &str) -> Option<String> {
        let v = Self::get_prompt_note(key);
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    }

    fn normalize_choice(value: Option<&str>, allowed: &[&str], default: &str) -> String {
        match value {
            Some(v) => {
                let text = v.trim().to_lowercase();
                if allowed.contains(&text.as_str()) {
                    text
                } else {
                    default.to_string()
                }
            }
            None => default.to_string(),
        }
    }

    fn parse_memory_profile(value: Option<&str>, default: MemoryProfile) -> MemoryProfile {
        match value {
            Some("off") => MemoryProfile::Off,
            Some("facts_only") => MemoryProfile::FactsOnly,
            Some("relevant") => MemoryProfile::Relevant,
            Some("rich") => MemoryProfile::Rich,
            _ => default,
        }
    }

    fn parse_section_intensity(
        raw: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> HashMap<String, SectionIntensity> {
        let mut map = HashMap::new();
        if let Some(obj) = raw {
            for (key, val) in obj {
                if let Some(s) = val.as_str() {
                    let intensity = match s {
                        "high" => SectionIntensity::High,
                        "normal" => SectionIntensity::Normal,
                        "light" => SectionIntensity::Light,
                        "off" => SectionIntensity::Off,
                        _ => continue,
                    };
                    map.insert(key.clone(), intensity);
                }
            }
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scope::ChatScope;

    #[test]
    fn test_decision_output_schema() {
        let pp = PromptPlanner::default();
        let schema = pp.decision_output_schema(false);
        assert!(schema.contains("reply_goal"));
        assert!(schema.contains("prompt_plan"));
        assert!(!schema.contains("emoji_should_send"));
    }

    #[test]
    fn test_decision_output_schema_emoji_enabled() {
        let pp = PromptPlanner::default();
        let schema = pp.decision_output_schema(true);
        assert!(schema.contains("emoji_should_send"));
    }

    #[test]
    fn test_default_prompt_plan_group() {
        let pp = PromptPlanner::default();
        let plan = pp.default_prompt_plan(true, "unknown", false, false);
        assert_eq!(plan.reply_goal, "light_presence");
        assert_eq!(plan.initiative, "reactive");
        assert_eq!(plan.tone_profile, "concise");
    }

    #[test]
    fn test_default_prompt_plan_private() {
        let pp = PromptPlanner::default();
        let plan = pp.default_prompt_plan(false, "soft_continuation", false, false);
        assert_eq!(plan.reply_goal, "answer");
        assert_eq!(plan.continuity_mode, "resume_recent_topic");
        assert_eq!(plan.tone_profile, "balanced");
    }

    #[test]
    fn test_default_prompt_plan_comfort() {
        let pp = PromptPlanner::default();
        let _plan = pp.default_prompt_plan(true, "unknown", false, false);
        // "comfort" reply_goal from a specific path - let's test old_topic_resume
        let plan2 = pp.default_prompt_plan(false, "old_topic_resume", false, false);
        assert_eq!(plan2.reply_goal, "recall");
        assert_eq!(plan2.memory_profile, MemoryProfile::Rich);
        assert!(plan2.policy.include_precise_recall);
    }

    #[test]
    fn test_normalize_choice() {
        assert_eq!(
            PromptPlanner::normalize_choice(Some("balanced"), &["balanced", "warm"], "balanced"),
            "balanced"
        );
        assert_eq!(
            PromptPlanner::normalize_choice(Some("unknown"), &["balanced", "warm"], "balanced"),
            "balanced"
        );
        assert_eq!(
            PromptPlanner::normalize_choice(None, &["balanced", "warm"], "balanced"),
            "balanced"
        );
    }

    #[test]
    fn test_parse_prompt_plan_no_decision() {
        let pp = PromptPlanner::default();
        // Test with None decision
        let result = pp.parse_prompt_plan(None, &make_event(), true, false);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_prompt_plan_action_not_reply() {
        let pp = PromptPlanner::default();
        let mut map = serde_json::Map::new();
        map.insert("action".into(), serde_json::Value::String("wait".into()));
        let result = pp.parse_prompt_plan(Some(&map), &make_event(), true, false);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_prompt_plan_default() {
        let pp = PromptPlanner::default();
        let mut map = serde_json::Map::new();
        map.insert("action".into(), serde_json::Value::String("reply".into()));
        // No prompt_plan key → returns default plan
        let result = pp.parse_prompt_plan(Some(&map), &make_event(), true, false);
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_prompt_plan_full() {
        let pp = PromptPlanner::default();
        let mut map = serde_json::Map::new();
        map.insert("action".into(), serde_json::Value::String("reply".into()));
        let prompt_plan = serde_json::json!({
            "reply_goal": "comfort",
            "continuity_mode": "resume_recent_topic",
            "timeline_detail": "per_message",
            "context_profile": "standard",
            "memory_profile": "relevant",
            "tone_profile": "warm",
            "initiative": "gentle_follow",
            "expression_profile": "companion",
            "policy": {
                "include_recent_history": true,
                "include_person_facts": false,
                "include_session_restore": true,
                "include_precise_recall": false,
                "include_dynamic_memory": true,
                "include_vision_context": true,
                "include_reply_scope": true,
                "include_style_guide": true
            },
            "notes": "test notes"
        });
        map.insert("prompt_plan".into(), prompt_plan);
        map.insert("emoji_should_send".into(), serde_json::Value::Bool(true));
        map.insert(
            "emoji_intent_reference".into(),
            serde_json::Value::String("happy face".into()),
        );

        let result = pp.parse_prompt_plan(Some(&map), &make_event(), false, false);
        let plan = result.unwrap();
        assert_eq!(plan.reply_goal, "comfort");
        assert_eq!(plan.tone_profile, "warm");
        assert!(!plan.policy.include_person_facts);
        assert!(plan.emoji_should_send);
        assert_eq!(plan.emoji_intent_reference, "happy face");
    }

    fn make_event() -> InboundEvent {
        InboundEvent {
            id: "e1".into(),
            platform: "test".into(),
            event_type: crate::core::platform_types::EventType::Message,
            message: Some(crate::core::types::UserMessage {
                id: "m1".into(),
                sender_id: "u1".into(),
                sender_name: "T".into(),
                text: "hi".into(),
                timestamp: chrono::Utc::now(),
                scope: ChatScope::Private,
                is_mention: false,
            }),
            raw_payload: None,
            received_at: chrono::Utc::now(),
            session: None,
            ..Default::default()
        }
    }
}
