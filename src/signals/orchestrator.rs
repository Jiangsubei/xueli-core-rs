use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing;

use crate::core::platform_types::InboundEvent;
use crate::memory::stores::signal_store::SignalStore;
use crate::prelude::XueliResult;
use crate::signals::cache::SignalCache;
use crate::signals::engagement::build_message_observations;
use crate::signals::temporal::TemporalContext;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};
use crate::traits::prompt_template::PromptTemplateLoader;

/// L1 缓存元数据（与 L2 同步比较用）
#[derive(Debug, Clone)]
struct L1Meta {
    updated_at: f64,
    signature: String,
}

/// 综合语义信号
#[derive(Debug, Clone)]
pub struct SemanticSignals {
    pub temporal: Option<TemporalContext>,
    pub metacognition: MetacognitionSignal,
    pub engagement: EngagementSignal,
    pub observations: ObservationsSignal,
}

/// 元认知信号
#[derive(Debug, Clone, Default)]
pub struct MetacognitionSignal {
    pub caution_level: String,
    pub caution_reasons: Vec<String>,
}

/// 互动参与信号
#[derive(Debug, Clone, Default)]
pub struct EngagementSignal {
    pub message_length: usize,
    pub question_count: usize,
    pub emoji_count: usize,
}

/// 消息观察信号（结构化可观测事实）
#[derive(Debug, Clone, Default)]
pub struct ObservationsSignal {
    pub message_length_bucket: String,
    pub is_short_message: bool,
    pub is_light_response_candidate: bool,
    pub is_continuation_candidate: bool,
    pub assistant_replied_recently: bool,
    pub follows_assistant_recently: bool,
    pub same_user_continuation: bool,
    pub recent_history_count: usize,
}

/// 语义信号编排器 — L1 缓存 → L2 存储 → LLM 计算 → 降级回退
///
/// 对应 Python 版 `xueli/src/handlers/signals/orchestrator.py`
pub struct SignalOrchestrator<A: AIClient, L: PromptTemplateLoader> {
    signal_store: Arc<SignalStore>,
    ai_client: Arc<A>,
    cache: Mutex<SignalCache<Value>>,
    template_loader: Arc<L>,
    model: String,
    locale: String,
    short_ttl_seconds: f64,
    prompt_version: String,
    image_scope_mode: String,
    l1_meta: Mutex<HashMap<String, L1Meta>>,
    l1_last_revalidate_at: Mutex<HashMap<String, Instant>>,
    l1_revalidate_interval_seconds: f64,
    last_cleanup_at: Mutex<Instant>,
    cleanup_lock: Mutex<()>,
}

/// 从事件中提取结构化信号（不含 LLM 调用）
pub fn extract_structured_signals(event: &InboundEvent) -> SemanticSignals {
    let (text, sender_id) = match &event.message {
        Some(msg) => (msg.text.as_str(), msg.sender_id.as_str()),
        None => ("", ""),
    };

    let engagement = EngagementSignal {
        message_length: text.chars().count(),
        question_count: text.matches('?').count() + text.matches('？').count(),
        emoji_count: count_emoji(text),
    };

    let observations = build_message_observations(text, sender_id, "", "", "unknown", 0);

    SemanticSignals {
        temporal: None,
        metacognition: MetacognitionSignal::default(),
        engagement,
        observations: ObservationsSignal {
            message_length_bucket: observations.message_length_bucket,
            is_short_message: observations.is_short_message,
            is_light_response_candidate: observations.is_light_response_candidate,
            is_continuation_candidate: observations.is_continuation_candidate,
            assistant_replied_recently: observations.assistant_replied_recently,
            follows_assistant_recently: observations.follows_assistant_recently,
            same_user_continuation: observations.same_user_continuation,
            recent_history_count: observations.recent_history_count,
        },
    }
}

impl<A: AIClient, L: PromptTemplateLoader> SignalOrchestrator<A, L> {
    pub fn new(
        signal_store: Arc<SignalStore>,
        ai_client: Arc<A>,
        template_loader: Arc<L>,
        model: &str,
        locale: &str,
        short_ttl_seconds: f64,
        prompt_version: &str,
        image_scope_mode: &str,
    ) -> Self {
        let short_ttl = short_ttl_seconds.max(1.0);
        let scope_mode = image_scope_mode.trim().to_lowercase();
        let scope_mode = if scope_mode.is_empty() {
            "global".to_string()
        } else {
            scope_mode
        };
        Self {
            signal_store,
            ai_client,
            cache: Mutex::new(SignalCache::new()),
            template_loader,
            model: model.to_string(),
            locale: locale.to_string(),
            short_ttl_seconds: short_ttl,
            prompt_version: prompt_version.to_string(),
            image_scope_mode: scope_mode,
            l1_meta: Mutex::new(HashMap::new()),
            l1_last_revalidate_at: Mutex::new(HashMap::new()),
            l1_revalidate_interval_seconds: 5.0,
            last_cleanup_at: Mutex::new(Instant::now()),
            cleanup_lock: Mutex::new(()),
        }
    }

    pub fn extract_structured(&self, event: &InboundEvent) -> SemanticSignals {
        extract_structured_signals(event)
    }

    // ── 信号 Key 构建 ──────────────────────────────────────────

    pub fn build_text_signal_key(
        &self,
        signal_type: &str,
        scope_key: &str,
        message_id: &str,
    ) -> String {
        format!(
            "{}:{}:{}:{}",
            signal_type,
            self.prompt_version,
            scope_key.trim(),
            message_id.trim()
        )
    }

    pub fn build_image_signal_key(
        &self,
        signal_type: &str,
        image_payloads: &[String],
        scope_key: &str,
    ) -> String {
        let mut hasher = Sha256::new();
        for payload in image_payloads {
            hasher.update(payload.as_bytes());
            hasher.update(b"|");
        }
        let digest = format!("{:x}", hasher.finalize());
        let scope_clean = scope_key.trim();
        match self.image_scope_mode.as_str() {
            "session" => {
                format!(
                    "{}:{}:session:{}:content_hash:{}",
                    signal_type, self.prompt_version, scope_clean, digest
                )
            }
            "user" => {
                let user_scope = scope_clean.split(':').last().unwrap_or("");
                format!(
                    "{}:{}:user:{}:content_hash:{}",
                    signal_type, self.prompt_version, user_scope, digest
                )
            }
            _ => {
                format!(
                    "{}:{}:content_hash:{}",
                    signal_type, self.prompt_version, digest
                )
            }
        }
    }

    // ── 视觉信号缓存读取 ─────────────────────────────────────

    pub async fn get_cached_vision_signal(
        &self,
        image_payloads: &[String],
        scope_key: &str,
    ) -> Option<Value> {
        self._maybe_cleanup().await;
        let key = self.build_image_signal_key("vision_signal", image_payloads, scope_key);
        {
            let mut cache = self.cache.lock().await;
            if let Some(cached) = cache.get(&key) {
                drop(cache);
                if self._is_l1_stale(&key).await {
                    let mut cache = self.cache.lock().await;
                    cache.pop(&key);
                    self.l1_meta.lock().await.remove(&key);
                } else {
                    tracing::debug!("[信号编排] L1命中: vision_signal");
                    return Some(cached);
                }
            }
        }
        if let Some(stored) = self.signal_store.get(&key).await {
            tracing::debug!("[信号编排] L2命中: vision_signal");
            {
                let mut cache = self.cache.lock().await;
                cache.set(
                    &key,
                    stored.clone(),
                    Duration::from_secs_f64(self.short_ttl_seconds),
                );
            }
            self._capture_l1_meta_from_l2(&key).await;
            return Some(stored);
        }
        None
    }

    // ── 回复后处理分流信号 ────────────────────────────────────

    pub async fn get_or_compute_feedback_triage_signal(
        &self,
        scope_key: &str,
        message_id: &str,
        reply_text: &str,
        user_text: &str,
        expected_effect: &str,
        predicted_response: &str,
        relationship_summary: &str,
    ) -> Value {
        self._maybe_cleanup().await;
        let key = self.build_text_signal_key("feedback_triage_signal", scope_key, message_id);

        {
            let mut cache = self.cache.lock().await;
            if let Some(cached) = cache.get(&key) {
                drop(cache);
                if self._is_l1_stale(&key).await {
                    let mut cache = self.cache.lock().await;
                    cache.pop(&key);
                    self.l1_meta.lock().await.remove(&key);
                } else {
                    return cached;
                }
            }
        }

        if let Some(stored) = self.signal_store.get(&key).await {
            {
                let mut cache = self.cache.lock().await;
                cache.set(
                    &key,
                    stored.clone(),
                    Duration::from_secs_f64(self.short_ttl_seconds),
                );
            }
            self._capture_l1_meta_from_l2(&key).await;
            return stored;
        }

        let computed = match self
            ._compute_feedback_triage_signal(
                reply_text,
                user_text,
                expected_effect,
                predicted_response,
                relationship_summary,
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!("[信号编排] feedback triage 计算失败，降级规则: {:?}", e);
                Value::Object(serde_json::Map::new())
            }
        };

        self._persist_then_keep_short_ttl(&key, "feedback_triage_signal", &computed)
            .await;
        computed
    }

    // ── 长期叙事记忆更新 ──────────────────────────────────────

    pub async fn compute_narrative_self_signal(&self, payload: &Value) -> Value {
        let system_prompt = match self
            .template_loader
            .get_template(&self.locale, "narrative_self.prompt")
            .await
        {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!("[信号编排] narrative_self.prompt 模板加载失败: {:?}", e);
                return Value::Object(serde_json::Map::new());
            }
        };

        let user_content = build_narrative_self_user_prompt(payload);

        let data = match self._invoke_text(&system_prompt, &user_content).await {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!("[信号编排] narrative_self 计算失败，跳过更新: {:?}", e);
                return Value::Object(serde_json::Map::new());
            }
        };

        let mut result = serde_json::Map::new();
        result.insert(
            "relationship_story".to_string(),
            Value::String(str_val(&data, "relationship_story")),
        );
        result.insert(
            "recurring_themes".to_string(),
            list_val(&data, "recurring_themes", 8),
        );
        result.insert(
            "recent_turning_points".to_string(),
            list_val(&data, "recent_turning_points", 8),
        );
        result.insert(
            "reply_guidance".to_string(),
            Value::String(str_val(&data, "reply_guidance")),
        );
        result.insert(
            "confidence".to_string(),
            Value::from(clamp_f64(f64_val(&data, "confidence"), 0.0, 1.0)),
        );
        result.insert(
            "reason".to_string(),
            Value::String(str_val(&data, "reason")),
        );
        Value::Object(result)
    }

    // ── 角色成长信号更新 ──────────────────────────────────────

    pub async fn compute_character_adaptation_signal(&self, payload: &Value) -> Value {
        let system_prompt = match self
            .template_loader
            .get_template(&self.locale, "character_adaptation.prompt")
            .await
        {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(
                    "[信号编排] character_adaptation.prompt 模板加载失败: {:?}",
                    e
                );
                return Value::Object(serde_json::Map::new());
            }
        };

        let user_content = serde_json::to_string(payload).unwrap_or_default();

        let data = match self._invoke_text(&system_prompt, &user_content).await {
            Ok(v) => v,
            Err(_) => {
                tracing::error!("[角色成长] LLM persona hints 计算失败");
                return Value::Object(serde_json::Map::new());
            }
        };

        let confidence = clamp_f64(f64_val(&data, "confidence"), 0.0, 1.0);
        let reason = str_val(&data, "reason");
        if !reason.is_empty() {
            tracing::debug!(
                "[角色成长] confidence={:.2}, reason={}",
                confidence,
                &reason[..reason.len().min(80)]
            );
        }

        let mut result = serde_json::Map::new();
        result.insert("core_traits".to_string(), list_val(&data, "core_traits", 8));
        result.insert(
            "tone_preferences".to_string(),
            list_val(&data, "tone_preferences", 8),
        );
        result.insert(
            "behavior_habits".to_string(),
            list_val(&data, "behavior_habits", 8),
        );
        result.insert(
            "bot_persona_hints".to_string(),
            list_val(&data, "bot_persona_hints", 8),
        );
        result.insert(
            "style_adaptation_summary".to_string(),
            Value::String(str_val(&data, "style_adaptation_summary")),
        );
        result.insert(
            "relationship_tone_hint".to_string(),
            Value::String(str_val(&data, "relationship_tone_hint")),
        );
        result.insert(
            "emotional_trend".to_string(),
            Value::String(str_val(&data, "emotional_trend")),
        );
        result.insert(
            "feedback_summary".to_string(),
            Value::String(str_val(&data, "feedback_summary")),
        );
        Value::Object(result)
    }

    // ── 视觉信号持久化 ────────────────────────────────────────

    pub async fn persist_vision_signal(
        &self,
        image_payloads: &[String],
        payload: &Value,
        scope_key: &str,
    ) -> XueliResult<()> {
        self._maybe_cleanup().await;
        let key = self.build_image_signal_key("vision_signal", image_payloads, scope_key);
        let confidence = clamp_f64(f64_val(payload, "vision_confidence"), 0.0, 1.0);
        {
            let mut cache = self.cache.lock().await;
            cache.set(
                &key,
                payload.clone(),
                Duration::from_secs_f64(self.short_ttl_seconds),
            );
        }
        let _ = self
            .signal_store
            .set(
                &key,
                "vision_signal",
                &self.prompt_version,
                payload,
                confidence,
                self.short_ttl_seconds,
            )
            .await;
        {
            let mut cache = self.cache.lock().await;
            cache.pop(&key);
            cache.set(
                &key,
                payload.clone(),
                Duration::from_secs_f64(self.short_ttl_seconds),
            );
        }
        self._capture_l1_meta_from_l2(&key).await;
        Ok(())
    }

    // ── 缓存失效 ──────────────────────────────────────────────

    pub async fn invalidate_signal_key(&self, signal_key: &str) {
        let key = signal_key.trim();
        if key.is_empty() {
            return;
        }
        self.cache.lock().await.pop(key);
        self.l1_meta.lock().await.remove(key);
        self.l1_last_revalidate_at.lock().await.remove(key);
        let _ = self.signal_store.invalidate_key(key).await;
    }

    pub async fn invalidate_signals_by_prefix(&self, prefix: &str) -> XueliResult<usize> {
        let normalized = prefix.trim();
        if normalized.is_empty() {
            return Ok(0);
        }
        {
            let l1_meta = self.l1_meta.lock().await;
            let keys: Vec<String> = l1_meta
                .keys()
                .filter(|k| k.starts_with(normalized))
                .cloned()
                .collect();
            drop(l1_meta);
            let mut cache = self.cache.lock().await;
            let mut meta = self.l1_meta.lock().await;
            let mut revalidate = self.l1_last_revalidate_at.lock().await;
            for k in &keys {
                cache.pop(k);
                meta.remove(k);
                revalidate.remove(k);
            }
        }
        self.signal_store.invalidate_prefix(normalized).await
    }

    // ── 内部方法 ──────────────────────────────────────────────

    async fn _maybe_cleanup(&self) {
        let now = Instant::now();
        let interval = Duration::from_secs_f64(self.short_ttl_seconds.max(5.0));
        {
            let last = self.last_cleanup_at.lock().await;
            if now.duration_since(*last) < interval {
                return;
            }
        }
        let _guard = self.cleanup_lock.lock().await;
        {
            let last = self.last_cleanup_at.lock().await;
            if Instant::now().duration_since(*last) < interval {
                return;
            }
        }
        {
            let mut cache = self.cache.lock().await;
            cache.cleanup();
        }
        let _ = self.signal_store.cleanup_expired().await;
        *self.last_cleanup_at.lock().await = Instant::now();
    }

    async fn _persist_then_keep_short_ttl(&self, key: &str, signal_type: &str, payload: &Value) {
        let confidence = clamp_f64(f64_val(payload, "confidence"), 0.0, 1.0);
        {
            let mut cache = self.cache.lock().await;
            cache.set(
                key,
                payload.clone(),
                Duration::from_secs_f64(self.short_ttl_seconds),
            );
        }
        let _ = self
            .signal_store
            .set(
                key,
                signal_type,
                &self.prompt_version,
                payload,
                confidence,
                self.short_ttl_seconds,
            )
            .await;
        {
            let mut cache = self.cache.lock().await;
            cache.pop(key);
            cache.set(
                key,
                payload.clone(),
                Duration::from_secs_f64(self.short_ttl_seconds),
            );
        }
        self._capture_l1_meta_from_l2(key).await;
    }

    async fn _capture_l1_meta_from_l2(&self, key: &str) {
        if let Some(meta) = self.signal_store.get_meta(key).await {
            self.l1_meta.lock().await.insert(
                key.to_string(),
                L1Meta {
                    updated_at: meta.updated_at,
                    signature: meta.signature,
                },
            );
            self.l1_last_revalidate_at
                .lock()
                .await
                .insert(key.to_string(), Instant::now());
        }
    }

    async fn _is_l1_stale(&self, key: &str) -> bool {
        let now = Instant::now();
        {
            let revalidate = self.l1_last_revalidate_at.lock().await;
            if let Some(last) = revalidate.get(key) {
                if now.duration_since(*last).as_secs_f64() < self.l1_revalidate_interval_seconds {
                    return false;
                }
            }
        }
        self.l1_last_revalidate_at
            .lock()
            .await
            .insert(key.to_string(), now);

        let cached_meta = self.l1_meta.lock().await.get(key).cloned();
        let l2_meta = self.signal_store.get_meta(key).await;

        let l2_meta = match l2_meta {
            Some(m) => m,
            None => return true,
        };

        match cached_meta {
            Some(ref cached) => {
                if l2_meta.updated_at != cached.updated_at || l2_meta.signature != cached.signature
                {
                    tracing::debug!("[信号编排] L1失效: key={}", &key[..key.len().min(80)]);
                    self.l1_meta.lock().await.insert(
                        key.to_string(),
                        L1Meta {
                            updated_at: l2_meta.updated_at,
                            signature: l2_meta.signature,
                        },
                    );
                    return true;
                }
                false
            }
            None => {
                self.l1_meta.lock().await.insert(
                    key.to_string(),
                    L1Meta {
                        updated_at: l2_meta.updated_at,
                        signature: l2_meta.signature,
                    },
                );
                false
            }
        }
    }

    async fn _invoke_text(&self, system_prompt: &str, user_content: &str) -> XueliResult<Value> {
        let messages = vec![
            ChatMessage::text("system", system_prompt),
            ChatMessage::text("user", user_content),
        ];

        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(0.1),
            max_tokens: Some(2048),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = self.ai_client.chat_completion(&request).await?;
        Ok(extract_json_object(&response.content))
    }

    async fn _compute_feedback_triage_signal(
        &self,
        reply_text: &str,
        user_text: &str,
        expected_effect: &str,
        predicted_response: &str,
        relationship_summary: &str,
    ) -> XueliResult<Value> {
        let system_prompt = self
            .template_loader
            .get_template(&self.locale, "feedback_triage.prompt")
            .await?;
        let user_content = build_feedback_triage_user_prompt(
            reply_text,
            user_text,
            expected_effect,
            predicted_response,
            relationship_summary,
        );

        let data = self._invoke_text(&system_prompt, &user_content).await?;

        let effect = str_val(&data, "actual_effect");
        let normalized_effect = normalize_effect(&effect);

        let mut result = serde_json::Map::new();
        result.insert(
            "reply_effect_label".to_string(),
            Value::String(str_val(&data, "reply_effect_label").to_lowercase()),
        );
        result.insert(
            "reply_effect_score".to_string(),
            Value::from(f64_val(&data, "reply_effect_score")),
        );
        result.insert(
            "actual_effect".to_string(),
            Value::String(normalized_effect),
        );
        result.insert(
            "expected_effect_met".to_string(),
            optional_bool_val(&data, "expected_effect_met"),
        );
        result.insert(
            "prediction_met".to_string(),
            optional_bool_val(&data, "prediction_met"),
        );
        result.insert(
            "style_feedback_label".to_string(),
            Value::String(str_val(&data, "style_feedback_label")),
        );
        result.insert(
            "emotion_label".to_string(),
            Value::String(str_val(&data, "emotion_label")),
        );
        result.insert(
            "confidence".to_string(),
            Value::from(clamp_f64(f64_val(&data, "confidence"), 0.0, 1.0)),
        );
        result.insert(
            "reason".to_string(),
            Value::String(str_val(&data, "reason")),
        );
        result.insert("warmth".to_string(), optional_float_val(&data, "warmth"));
        result.insert(
            "appropriateness".to_string(),
            optional_float_val(&data, "appropriateness"),
        );
        result.insert(
            "social_presence".to_string(),
            optional_float_val(&data, "social_presence"),
        );
        result.insert(
            "version".to_string(),
            Value::String(self.prompt_version.clone()),
        );
        Ok(Value::Object(result))
    }
}

// ── 辅助函数 ───────────────────────────────────────────────

fn str_val(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn f64_val(data: &Value, key: &str) -> f64 {
    data.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

fn clamp_f64(v: f64, min: f64, max: f64) -> f64 {
    v.clamp(min, max)
}

fn list_val(data: &Value, key: &str, max_items: usize) -> Value {
    let arr: Vec<Value> = data
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| {
                    let s = v.as_str().unwrap_or("").trim().to_string();
                    if s.is_empty() {
                        None
                    } else {
                        Some(Value::String(s))
                    }
                })
                .take(max_items)
                .collect()
        })
        .unwrap_or_default();
    Value::Array(arr)
}

fn normalize_effect(value: &str) -> String {
    let norm = value.trim().to_lowercase();
    match norm.as_str() {
        "continue" | "satisfy" | "cool_down" | "clarify" | "none" => norm,
        _ => "none".to_string(),
    }
}

fn optional_bool_val(data: &Value, key: &str) -> Value {
    match data.get(key).and_then(|v| v.as_bool()) {
        Some(b) => Value::Bool(b),
        None => Value::Null,
    }
}

fn optional_float_val(data: &Value, key: &str) -> Value {
    match data.get(key).and_then(|v| v.as_f64()) {
        Some(v) => Value::from(clamp_f64(v, 0.0, 1.0)),
        None => Value::Null,
    }
}

/// 构建 narrative_self 的用户提示
fn build_narrative_self_user_prompt(payload: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(old) = payload.get("old_narrative_self") {
        if let Some(story) = old.get("relationship_story").and_then(|v| v.as_str()) {
            if !story.trim().is_empty() {
                parts.push(format!("【旧相处脉络】\n{}", story));
            }
        }
    }

    if let Some(msgs) = payload.get("recent_messages").and_then(|v| v.as_array()) {
        let mut msg_lines: Vec<String> = Vec::new();
        for item in msgs {
            let speaker = item
                .get("speaker")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if !speaker.is_empty() && !text.is_empty() {
                msg_lines.push(format!("{}: {}", speaker, text));
            }
        }
        if !msg_lines.is_empty() {
            parts.push(format!("【近期对话】\n{}", msg_lines.join("\n")));
        }
    }

    if let Some(ups) = payload.get("user_profile_signal") {
        if let Some(obj) = ups.as_object() {
            let mut up_lines: Vec<String> = Vec::new();
            if let Some(style) = obj.get("style_summary").and_then(|v| v.as_str()) {
                if !style.trim().is_empty() {
                    up_lines.push(format!("风格={}", style.trim()));
                }
            }
            if let Some(tl) = obj.get("trust_level").and_then(|v| v.as_f64()) {
                up_lines.push(format!("信任度={:.2}", tl));
            }
            if !up_lines.is_empty() {
                parts.push(format!("【用户画像】\n{}", up_lines.join("\n")));
            }
        }
    }

    let fb = str_val(payload, "feedback_summary");
    if !fb.is_empty() {
        parts.push(format!("【反馈摘要】\n{}", fb));
    }

    let pf = str_val(payload, "person_fact_context");
    if !pf.is_empty() {
        parts.push(format!("【人物事实】\n{}", pf));
    }

    let ct = str_val(payload, "current_thread_summary");
    if !ct.is_empty() {
        parts.push(format!("【当前话题】\n{}", ct));
    }

    parts.join("\n\n")
}

/// 构建 feedback_triage 的用户提示
fn build_feedback_triage_user_prompt(
    reply_text: &str,
    user_text: &str,
    expected_effect: &str,
    predicted_response: &str,
    relationship_summary: &str,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    let rel = relationship_summary.trim();
    if !rel.is_empty() {
        parts.push(format!("【当前关系】\n{}", rel));
    }

    let reply = reply_text.trim();
    if !reply.is_empty() {
        parts.push(format!("【助手回复】\n{}", reply));
    }

    let user = user_text.trim();
    if !user.is_empty() {
        parts.push(format!("【用户反馈】\n{}", user));
    }

    if !expected_effect.is_empty() {
        let effect_labels: &[(&str, &str)] = &[
            ("continue", "继续话题"),
            ("satisfy", "满足需求"),
            ("cool_down", "缓和情绪"),
            ("clarify", "澄清信息"),
        ];
        let label = effect_labels
            .iter()
            .find(|(k, _)| *k == expected_effect)
            .map(|(_, v)| *v)
            .unwrap_or(expected_effect);
        parts.push(format!("【预期效果】\n{}", label));
    } else {
        parts.push("【预期效果】\n（无）".to_string());
    }

    let pred = predicted_response.trim();
    if !pred.is_empty() {
        parts.push(format!("【预测反应】\n{}", pred));
    } else {
        parts.push("【预测反应】\n（无）".to_string());
    }

    parts.join("\n\n")
}

/// 从 LLM 回复文本中提取 JSON 对象
fn extract_json_object(content: &str) -> Value {
    let text = content.trim();
    if text.is_empty() {
        return Value::Object(serde_json::Map::new());
    }

    // 直接解析
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if v.is_object() {
            return v;
        }
    }

    // 匹配 fenced code block: ```json { ... } ```
    if let Ok(re) = Regex::new(r"(?si)```(?:json)?\s*\n?(\{.*?\})\s*\n?```") {
        if let Some(caps) = re.captures(text) {
            if let Some(m) = caps.get(1) {
                if let Ok(v) = serde_json::from_str::<Value>(m.as_str()) {
                    if v.is_object() {
                        return v;
                    }
                }
            }
        }
    }

    // 匹配裸 JSON 对象：找第一个 { 和对应的 }
    if let Some(start) = text.find('{') {
        let bytes = text.as_bytes();
        let mut depth = 0i32;
        let mut end_idx = text.len();
        for i in start..bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end_idx = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        if depth == 0 {
            let json_str = &text[start..end_idx];
            if let Ok(v) = serde_json::from_str::<Value>(json_str) {
                if v.is_object() {
                    return v;
                }
            }
        }
    }

    Value::Object(serde_json::Map::new())
}

/// 统计文本中的 emoji 数量
fn count_emoji(text: &str) -> usize {
    text.chars()
        .filter(|c| {
            let cp = *c as u32;
            (0x1F600..=0x1F64F).contains(&cp)
                || (0x1F300..=0x1F5FF).contains(&cp)
                || (0x1F680..=0x1F6FF).contains(&cp)
                || (0x2600..=0x26FF).contains(&cp)
                || (0x2700..=0x27BF).contains(&cp)
                || (0x1F900..=0x1F9FF).contains(&cp)
                || (0x1FA00..=0x1FA6F).contains(&cp)
                || (0x1FA70..=0x1FAFF).contains(&cp)
                || (0xFE00..=0xFE0F).contains(&cp)
                || cp == 0x200D
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::platform_types::EventType;
    use crate::core::types::UserMessage;
    use chrono::Utc;

    fn make_event(text: &str) -> InboundEvent {
        InboundEvent {
            id: "msg1".to_string(),
            platform: "test".to_string(),
            event_type: EventType::Message,
            message: Some(UserMessage {
                id: "msg1".to_string(),
                sender_id: "user1".to_string(),
                sender_name: "测试用户".to_string(),
                text: text.to_string(),
                timestamp: Utc::now(),
                scope: crate::core::scope::ChatScope::Private,
                is_mention: false,
            }),
            raw_payload: None,
            received_at: Utc::now(),
            session: None,
            ..Default::default()
        }
    }

    #[test]
    fn test_extract_signals() {
        let event = make_event("你好吗？今天天气很好呢！");
        let signals = extract_structured_signals(&event);
        assert_eq!(signals.engagement.question_count, 1);
        assert!(signals.engagement.message_length > 0);
    }

    #[test]
    fn test_extract_signals_empty() {
        let event = make_event("");
        let signals = extract_structured_signals(&event);
        assert_eq!(signals.engagement.message_length, 0);
        assert_eq!(signals.engagement.question_count, 0);
    }

    #[test]
    fn test_extract_observations_short_message() {
        let event = make_event("嗯");
        let signals = extract_structured_signals(&event);
        assert!(signals.observations.is_short_message);
    }

    #[test]
    fn test_build_text_signal_key() {
        // Use a minimal orchestrator to test key building
        let store = Arc::new(SignalStore::new(":memory:").unwrap());
        let ai = Arc::new(TestAIClient);
        let loader = Arc::new(TestTemplateLoader);
        let orch = SignalOrchestrator::new(
            store,
            ai,
            loader,
            "test-model",
            "zh-CN",
            60.0,
            "v1",
            "global",
        );
        let key = orch.build_text_signal_key("feedback_triage_signal", "sess:123", "msg:456");
        assert_eq!(key, "feedback_triage_signal:v1:sess:123:msg:456");
    }

    #[test]
    fn test_build_image_signal_key_global() {
        let store = Arc::new(SignalStore::new(":memory:").unwrap());
        let ai = Arc::new(TestAIClient);
        let loader = Arc::new(TestTemplateLoader);
        let orch =
            SignalOrchestrator::new(store, ai, loader, "test", "zh-CN", 60.0, "v1", "global");
        let key = orch.build_image_signal_key(
            "vision_signal",
            &["img1".to_string(), "img2".to_string()],
            "scope",
        );
        // "img1|" + "img2|" SHA256
        let mut hasher = Sha256::new();
        hasher.update(b"img1");
        hasher.update(b"|");
        hasher.update(b"img2");
        hasher.update(b"|");
        let digest = format!("{:x}", hasher.finalize());
        assert_eq!(key, format!("vision_signal:v1:content_hash:{}", digest));
    }

    #[test]
    fn test_extract_json_object_direct() {
        let input = r#"{"key": "value", "num": 42}"#;
        let result = extract_json_object(input);
        assert_eq!(result["key"], "value");
        assert_eq!(result["num"], 42);
    }

    #[test]
    fn test_extract_json_object_fenced() {
        let input = "```json\n{\"a\": 1}\n```";
        let result = extract_json_object(input);
        assert_eq!(result["a"], 1);
    }

    #[test]
    fn test_extract_json_object_bare() {
        let input = "prefix text {\"b\": 2} suffix";
        let result = extract_json_object(input);
        assert_eq!(result["b"], 2);
    }

    #[test]
    fn test_extract_json_object_empty() {
        let result = extract_json_object("");
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_normalize_effect_known() {
        assert_eq!(normalize_effect("Continue"), "continue");
        assert_eq!(normalize_effect("SATISFY"), "satisfy");
        assert_eq!(normalize_effect("cool_down"), "cool_down");
    }

    #[test]
    fn test_normalize_effect_unknown() {
        assert_eq!(normalize_effect("something_else"), "none");
    }

    #[test]
    fn test_build_narrative_self_user_prompt() {
        let payload = serde_json::json!({
            "old_narrative_self": {
                "relationship_story": "长期关系叙述"
            },
            "recent_messages": [
                {"speaker": "用户", "text": "你好"},
                {"speaker": "助手", "text": "你好！"}
            ],
            "feedback_summary": "反馈摘要内容",
            "current_thread_summary": "当前话题内容"
        });
        let result = build_narrative_self_user_prompt(&payload);
        assert!(result.contains("【旧相处脉络】"));
        assert!(result.contains("长期关系叙述"));
        assert!(result.contains("【近期对话】"));
        assert!(result.contains("用户: 你好"));
        assert!(result.contains("【反馈摘要】"));
        assert!(result.contains("【当前话题】"));
    }

    #[test]
    fn test_build_feedback_triage_user_prompt() {
        let result = build_feedback_triage_user_prompt(
            "助手回复文本",
            "用户反馈文本",
            "continue",
            "预测反应文本",
            "关系摘要文本",
        );
        assert!(result.contains("【当前关系】"));
        assert!(result.contains("关系摘要文本"));
        assert!(result.contains("【助手回复】"));
        assert!(result.contains("助手回复文本"));
        assert!(result.contains("【用户反馈】"));
        assert!(result.contains("【预期效果】"));
        assert!(result.contains("继续话题"));
        assert!(result.contains("【预测反应】"));
    }

    #[test]
    fn test_build_feedback_triage_user_prompt_empty_fields() {
        let result = build_feedback_triage_user_prompt("", "", "", "", "");
        assert!(result.contains("【预期效果】\n（无）"));
        assert!(result.contains("【预测反应】\n（无）"));
    }

    // ── 测试用桩 ──

    use crate::traits::ai_client::ChatCompletionResponse;
    use async_trait::async_trait;
    use std::collections::HashMap as StdHashMap;

    struct TestAIClient;

    #[async_trait]
    impl AIClient for TestAIClient {
        async fn chat_completion(
            &self,
            _request: &ChatCompletionRequest,
        ) -> XueliResult<ChatCompletionResponse> {
            Ok(ChatCompletionResponse {
                content: r#"{"key": "test_response"}"#.to_string(),
                segments: None,
                reasoning_content: String::new(),
                finish_reason: "stop".to_string(),
                usage: None,
                model: "test".to_string(),
                tool_calls: None,
                raw_content: String::new(),
                raw_response: None,
            })
        }
    }

    struct TestTemplateLoader;

    impl PromptTemplateLoader for TestTemplateLoader {
        async fn load_templates(&self, _locale: &str) -> XueliResult<StdHashMap<String, String>> {
            Ok(StdHashMap::new())
        }

        async fn get_template(&self, _locale: &str, _name: &str) -> XueliResult<String> {
            Ok("test template content".to_string())
        }
    }
}
