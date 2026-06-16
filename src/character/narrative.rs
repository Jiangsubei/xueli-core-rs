use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::character::{build_scope_payload_path, legacy_payload_path};
use crate::prelude::XueliResult;

/// 叙事线 — 追踪长期相处中的叙事脉络
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrativeThread {
    pub id: String,
    pub user_id: String,
    pub theme: String,
    pub summary: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub events: Vec<NarrativeEvent>,
    pub turn_count_since_last_update: usize,
}

/// 叙事事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrativeEvent {
    pub timestamp: DateTime<Utc>,
    pub description: String,
    pub significance: f64,
}

/// 结构化叙事自我描述
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NarrativeSelf {
    #[serde(default)]
    pub relationship_story: String,
    #[serde(default)]
    pub recurring_themes: Vec<String>,
    #[serde(default)]
    pub recent_turning_points: Vec<String>,
    #[serde(default)]
    pub reply_guidance: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NarrativeSelfPayload {
    #[serde(default)]
    narrative_self: String,
    #[serde(default)]
    updated_at: String,
}

/// 叙事服务 — 管理每个用户的叙事线。
pub struct NarrativeService {
    threads: Mutex<HashMap<String, NarrativeThread>>,
    update_interval: usize,
    storage_dir: String,
    narrative_self: Mutex<Option<String>>,
    narrative_self_user: Mutex<Option<String>>,
}

impl NarrativeThread {
    pub fn new(user_id: &str, theme: &str) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: user_id.to_string(),
            theme: theme.to_string(),
            summary: String::new(),
            created_at: now,
            updated_at: now,
            events: Vec::new(),
            turn_count_since_last_update: 0,
        }
    }
}

impl NarrativeService {
    pub fn new(storage_dir: &str) -> Self {
        Self {
            threads: Mutex::new(HashMap::new()),
            update_interval: 10,
            storage_dir: storage_dir.to_string(),
            narrative_self: Mutex::new(None),
            narrative_self_user: Mutex::new(None),
        }
    }

    pub fn get_thread(&self, user_id: &str) -> NarrativeThread {
        let mut threads = self.threads.lock().unwrap();
        threads
            .entry(user_id.to_string())
            .or_insert_with(|| Self::load_or_create(user_id, &self.storage_dir))
            .clone()
    }

    pub fn add_event(&self, user_id: &str, description: &str, significance: f64) {
        let data_str = {
            let mut threads = self.threads.lock().unwrap();
            let thread = threads
                .entry(user_id.to_string())
                .or_insert_with(|| Self::load_or_create(user_id, &self.storage_dir));
            thread.events.push(NarrativeEvent {
                timestamp: Utc::now(),
                description: description.to_string(),
                significance,
            });
            thread.turn_count_since_last_update += 1;
            thread.updated_at = Utc::now();
            if thread.events.len() > 200 {
                thread.events.drain(..thread.events.len() - 200);
            }
            serde_json::to_string_pretty(thread).ok()
        };
        if let Some(data) = data_str {
            let path = self.thread_path(user_id);
            let tmp = format!("{}.tmp", path);
            let _ = std::fs::write(&tmp, &data);
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    pub fn update_summary(&self, user_id: &str, summary: &str) {
        let truncated = Self::build_summary(summary);
        let data_str = {
            let mut threads = self.threads.lock().unwrap();
            let thread = threads
                .entry(user_id.to_string())
                .or_insert_with(|| Self::load_or_create(user_id, &self.storage_dir));
            thread.summary = truncated;
            thread.updated_at = Utc::now();
            serde_json::to_string_pretty(thread).ok()
        };
        if let Some(data) = data_str {
            let path = self.thread_path(user_id);
            let tmp = format!("{}.tmp", path);
            let _ = std::fs::write(&tmp, &data);
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn build_summary(raw: &str) -> String {
        let compact: String = raw.split_whitespace().collect::<Vec<&str>>().join(" ");
        if compact.is_empty() {
            return String::new();
        }
        if compact.chars().count() <= 80 {
            return compact;
        }
        let truncated: String = compact.chars().take(80).collect();
        truncated.trim_end().to_string() + "..."
    }

    pub fn should_update(&self, user_id: &str, min_interval_seconds: f64) -> bool {
        let thread = self.get_thread(user_id);
        let turn_ok = thread.turn_count_since_last_update >= self.update_interval;
        let elapsed = Utc::now()
            .signed_duration_since(thread.updated_at)
            .num_seconds() as f64;
        let time_ok = elapsed >= min_interval_seconds;
        turn_ok && time_ok
    }

    /// 判断是否需要更新叙事自我（基于轮次或时间间隔）
    pub fn should_update_narrative_self(
        &self,
        user_id: &str,
        min_turns: usize,
        min_interval_seconds: f64,
    ) -> bool {
        let thread = self.get_thread(user_id);
        let effective_min_turns = min_turns.max(1);
        if thread.turn_count_since_last_update >= effective_min_turns {
            return true;
        }
        let ns = self.narrative_self.lock().unwrap();
        if ns.is_none() {
            // 没有叙事自我，检查是否有摘要
            return !thread.summary.trim().is_empty();
        }
        drop(ns);

        // 检查叙事自我的更新时间
        let path = build_scope_payload_path(&self.storage_dir, user_id, "narrative_self.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(payload) = serde_json::from_str::<NarrativeSelfPayload>(&data) {
                if !payload.updated_at.is_empty() {
                    if let Ok(last) = chrono::DateTime::parse_from_rfc3339(&payload.updated_at) {
                        let elapsed = Utc::now().signed_duration_since(last).num_seconds() as f64;
                        return elapsed >= min_interval_seconds.max(1.0);
                    }
                }
            }
        }
        true
    }

    pub fn mark_updated(&self, user_id: &str) {
        let mut threads = self.threads.lock().unwrap();
        if let Some(thread) = threads.get_mut(user_id) {
            thread.turn_count_since_last_update = 0;
        }
    }

    /// 获取叙事线程摘要 — 合并主题和摘要为单行描述。
    ///
    /// 对应上下文构建阶段的叙事线索注入。
    pub fn get_thread_summary(&self, user_id: &str) -> Option<String> {
        let thread = self.get_thread(user_id);
        let has_theme = !thread.theme.is_empty() && thread.theme != "default";
        let has_summary = !thread.summary.is_empty();

        if has_theme && has_summary {
            Some(format!("{}：{}", thread.theme, thread.summary))
        } else if has_summary {
            Some(thread.summary)
        } else if has_theme {
            Some(format!("当前叙事主题：{}", thread.theme))
        } else {
            None
        }
    }

    /// 归一化叙事自我数据，提取有效字段并限制长度
    pub fn normalize_narrative_self(value: &serde_json::Value) -> Option<NarrativeSelf> {
        let story = value
            .get("relationship_story")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let guidance = value
            .get("reply_guidance")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let themes: Vec<String> = value
            .get("recurring_themes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let s = v.as_str().unwrap_or("").trim().to_string();
                        if s.is_empty() {
                            None
                        } else {
                            Some(s)
                        }
                    })
                    .take(8)
                    .collect()
            })
            .unwrap_or_default();
        let turning_points: Vec<String> = value
            .get("recent_turning_points")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let s = v.as_str().unwrap_or("").trim().to_string();
                        if s.is_empty() {
                            None
                        } else {
                            Some(s)
                        }
                    })
                    .take(8)
                    .collect()
            })
            .unwrap_or_default();

        if story.is_empty() && themes.is_empty() && turning_points.is_empty() && guidance.is_empty()
        {
            return None;
        }

        let confidence = value
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let reason = value
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        Some(NarrativeSelf {
            relationship_story: story,
            recurring_themes: themes,
            recent_turning_points: turning_points,
            reply_guidance: guidance,
            confidence,
            reason,
            updated_at: String::new(),
        })
    }

    /// 提取标签（当前未实现，始终返回空字符串）
    #[allow(dead_code)]
    pub fn pick_label(_user_message: &str) -> String {
        String::new()
    }

    pub fn update_narrative_self(&self, user_id: &str, signal_text: &str) -> XueliResult<()> {
        {
            let mut ns = self.narrative_self.lock().unwrap();
            *ns = Some(signal_text.to_string());
            let mut nsu = self.narrative_self_user.lock().unwrap();
            *nsu = Some(user_id.to_string());
        }
        let payload = NarrativeSelfPayload {
            narrative_self: signal_text.to_string(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        let path = build_scope_payload_path(&self.storage_dir, user_id, "narrative_self.json");
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content = serde_json::to_string_pretty(&payload).unwrap_or_default();
        let tmp = format!("{}.tmp", path);
        let _ = std::fs::write(&tmp, &content);
        let _ = std::fs::rename(&tmp, &path);
        Ok(())
    }

    pub fn get_narrative_self(&self) -> Option<String> {
        self.narrative_self.lock().unwrap().clone()
    }

    pub fn load_narrative_self(&self, user_id: &str) -> Option<String> {
        let path = build_scope_payload_path(&self.storage_dir, user_id, "narrative_self.json");
        match std::fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str::<NarrativeSelfPayload>(&data) {
                Ok(p) if !p.narrative_self.is_empty() => return Some(p.narrative_self),
                _ => {}
            },
            Err(_) => {}
        }
        let legacy = legacy_payload_path(&self.storage_dir, user_id, "narrative_self.json");
        match std::fs::read_to_string(&legacy) {
            Ok(data) => serde_json::from_str::<NarrativeSelfPayload>(&data)
                .ok()
                .and_then(|p| {
                    if p.narrative_self.is_empty() {
                        None
                    } else {
                        Some(p.narrative_self)
                    }
                }),
            Err(_) => None,
        }
    }

    // ── async variants ──

    pub async fn update_narrative_self_async(
        &self,
        user_id: String,
        signal_text: String,
    ) -> XueliResult<()> {
        tokio::task::block_in_place(|| self.update_narrative_self(&user_id, &signal_text))
    }

    pub async fn get_narrative_self_async(&self) -> Option<String> {
        tokio::task::block_in_place(|| self.get_narrative_self())
    }

    fn thread_path(&self, user_id: &str) -> String {
        build_scope_payload_path(&self.storage_dir, user_id, "narrative.json")
    }

    fn load_or_create(user_id: &str, dir: &str) -> NarrativeThread {
        let path = build_scope_payload_path(dir, user_id, "narrative.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(thread) = serde_json::from_str::<NarrativeThread>(&data) {
                return thread;
            }
        }
        let legacy = legacy_payload_path(dir, user_id, "narrative.json");
        if let Ok(data) = std::fs::read_to_string(&legacy) {
            if let Ok(thread) = serde_json::from_str::<NarrativeThread>(&data) {
                return thread;
            }
        }
        NarrativeThread::new(user_id, "default")
    }

    #[allow(dead_code)]
    fn save_one(&self, user_id: &str) {
        let data_str = {
            let threads = self.threads.lock().unwrap();
            threads
                .get(user_id)
                .and_then(|t| serde_json::to_string_pretty(t).ok())
        };
        if let Some(data) = data_str {
            let path = self.thread_path(user_id);
            let tmp = format!("{}.tmp", path);
            let _ = std::fs::write(&tmp, &data);
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

impl Default for NarrativeService {
    fn default() -> Self {
        Self::new("data/narratives")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_service() -> NarrativeService {
        let dir = tempfile::TempDir::new().unwrap();
        NarrativeService::new(dir.path().to_str().unwrap())
    }

    #[test]
    fn test_new_thread() {
        let thread = NarrativeThread::new("u1", "学习编程");
        assert_eq!(thread.user_id, "u1");
        assert_eq!(thread.theme, "学习编程");
        assert_eq!(thread.turn_count_since_last_update, 0);
    }

    #[test]
    fn test_add_event() {
        let svc = make_service();
        svc.add_event("u1", "用户开始学习 Rust", 0.6);
        let thread = svc.get_thread("u1");
        assert_eq!(thread.events.len(), 1);
        assert_eq!(thread.turn_count_since_last_update, 1);
    }

    #[test]
    fn test_should_update() {
        let svc = make_service();
        assert!(!svc.should_update("u1", 1800.0));

        for i in 0..15 {
            svc.add_event("u1", &format!("对话轮次 {}", i), 0.3);
        }
        assert!(svc.should_update("u1", 0.0));

        svc.mark_updated("u1");
        assert!(!svc.should_update("u1", 0.0));
    }

    #[test]
    fn test_should_update_respects_interval() {
        let svc = make_service();
        for i in 0..15 {
            svc.add_event("u1", &format!("对话轮次 {}", i), 0.3);
        }
        assert!(!svc.should_update("u1", 999999.0));
        assert!(svc.should_update("u1", 0.0));
    }

    #[test]
    fn test_build_summary_truncation() {
        let long = "这是非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常长的消息文本用来测试摘要截断功能超过限制";
        let result = NarrativeService::build_summary(long);
        assert!(result.chars().count() <= 83); // 80 chars + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_build_summary_short() {
        let short = "简短消息";
        let result = NarrativeService::build_summary(short);
        assert_eq!(result, short);
    }

    #[test]
    fn test_build_summary_empty() {
        let result = NarrativeService::build_summary("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_summary_exactly_80() {
        let exact = "一二三四五六七八九十一二三四五六七八九十一二三四五六七八九十一二三四五六七八九十一二三四五六七八九十一二三四五六七八九十一二三四五六七八九十一二三四五六七八九十";
        let result = NarrativeService::build_summary(exact);
        assert!(result.chars().count() <= 83);
    }

    #[test]
    fn test_update_summary_truncation() {
        let svc = make_service();
        svc.update_summary(
            "u1",
            "这是非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常非常长的消息文本用来测试摘要截断功能超过限制",
        );
        let thread = svc.get_thread("u1");
        assert!(thread.summary.chars().count() <= 83);
        assert!(thread.summary.ends_with("..."));
    }

    #[test]
    fn test_narrative_self_persist() {
        let svc = make_service();
        svc.update_narrative_self("u1", "关系正在深入发展").unwrap();
        let ns = svc.get_narrative_self();
        assert_eq!(ns, Some("关系正在深入发展".to_string()));
    }

    #[test]
    fn test_narrative_self_load() {
        let svc = make_service();
        svc.update_narrative_self("u2", "用户对我越来越信任")
            .unwrap();
        let loaded = svc.load_narrative_self("u2");
        assert_eq!(loaded, Some("用户对我越来越信任".to_string()));
    }

    #[test]
    fn test_narrative_self_none_for_unknown_user() {
        let svc = make_service();
        let loaded = svc.load_narrative_self("unknown_user");
        assert_eq!(loaded, None);
    }

    #[test]
    fn test_scope_based_narrative_path() {
        let dir = "/tmp/test_narratives";
        let user_id = "qq:group:g123:u456";
        let path = build_scope_payload_path(dir, user_id, "narrative.json");
        assert!(path.contains("qq_group_g123_u456"));
        assert!(path.contains("narrative.json"));
    }
}
