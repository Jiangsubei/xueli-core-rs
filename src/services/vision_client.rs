use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::core::config::XueliConfig;
use crate::prelude::XueliResult;
use crate::services::invocation_router::{InvocationTask, ModelInvocationRouter};
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};
use crate::traits::prompt_template::PromptTemplateLoader;

const DEFAULT_EMOTION_LABELS: &[&str] = &[
    "开心", "喜欢", "惊讶", "无语", "委屈", "生气", "伤心", "嘲讽", "害怕", "困惑", "焦虑", "疲惫",
    "温暖", "内疚", "感动",
];

const DEFAULT_REPLY_TONES: &[&str] = &[
    "安慰", "附和", "吐槽", "庆祝", "调侃", "拒绝", "提醒", "收尾",
];

const FALLBACK_VISION_SYSTEM_PROMPT: &str = "你是图片理解助手。输出简洁可靠的 JSON。\n\n【输出格式】\n{\"images\":[{\"description\":\"描述\",\"is_sticker\":false,\"sticker_confidence\":0.0,\"sticker_reason\":\"依据\"}],\"merged_description\":\"整体摘要\"}\n不要输出 JSON 以外的内容。";
const FALLBACK_VISION_USER_PROMPT: &str =
    "{image_count_line}\n{user_text_line}\n优先依据图片本身可见内容作答";
const FALLBACK_VISION_EMOTION_PROMPT: &str = "你是表情包分类助手。\n【情绪标签】\n{emotion_labels}\n【回复语气标签】\n{reply_tones}\n【输出格式】\n{\"primary_emotion\":\"...\",\"confidence\":0.0,\"all_emotions\":[...],\"reply_tones\":[...],\"reply_intents\":[\"语气-情绪\"],\"reason\":\"...\",\"secondary_emotions\":[...],\"intensity\":0.0}\n不要输出 JSON 以外的内容。";
const FALLBACK_VISION_STICKER_PROMPT: &str = "请为这张表情包输出情绪和适合的回复场景。";

/// 贴纸情绪分类结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StickerEmotionResult {
    pub primary_emotion: String,
    pub all_emotions: Vec<String>,
    pub reply_tones: Vec<String>,
    pub reply_intents: Vec<String>,
    pub confidence: f64,
    pub reason: String,
    pub intensity: f64,
}

impl StickerEmotionResult {
    pub fn parse_failed() -> Self {
        Self {
            primary_emotion: "neutral".to_string(),
            all_emotions: Vec::new(),
            reply_tones: Vec::new(),
            reply_intents: Vec::new(),
            confidence: 0.0,
            reason: "parse_failed".to_string(),
            intensity: 0.0,
        }
    }
}

/// 图片分析结果
#[derive(Debug, Clone)]
pub struct ImageAnalysisResult {
    pub per_image_descriptions: Vec<String>,
    pub merged_description: String,
    pub sticker_flags: Vec<bool>,
    pub sticker_emotion_labels: Vec<String>,
    pub sticker_confidences: Vec<f64>,
    pub sticker_reasons: Vec<String>,
    pub success_count: usize,
    pub failure_count: usize,
    pub source: String,
    pub error: Option<String>,
}

impl ImageAnalysisResult {
    pub fn empty() -> Self {
        Self {
            per_image_descriptions: Vec::new(),
            merged_description: String::new(),
            sticker_flags: Vec::new(),
            sticker_emotion_labels: Vec::new(),
            sticker_confidences: Vec::new(),
            sticker_reasons: Vec::new(),
            success_count: 0,
            failure_count: 0,
            source: String::new(),
            error: None,
        }
    }

    pub fn has_usable_description(&self) -> bool {
        !self.merged_description.trim().is_empty()
            || self
                .per_image_descriptions
                .iter()
                .any(|d| !d.trim().is_empty())
    }

    /// 判断指定索引的图片是否为表情贴纸
    pub fn is_sticker(&self, index: usize) -> bool {
        self.sticker_flags.get(index).copied().unwrap_or(false)
    }

    /// 获取指定索引图片的贴纸识别置信度
    pub fn get_sticker_confidence(&self, index: usize) -> f64 {
        self.sticker_confidences.get(index).copied().unwrap_or(0.0)
    }

    /// 获取指定索引图片被识别为贴纸的原因
    pub fn get_sticker_reason(&self, index: usize) -> String {
        self.sticker_reasons.get(index).cloned().unwrap_or_default()
    }

    /// 获取指定索引图片的描述文本
    pub fn get_description(&self, index: usize) -> String {
        self.per_image_descriptions
            .get(index)
            .cloned()
            .unwrap_or_default()
    }

    pub fn to_prompt_fields(&self) -> HashMap<String, String> {
        let mut fields = HashMap::new();
        if !self.merged_description.trim().is_empty() {
            fields.insert(
                "merged_description".to_string(),
                self.merged_description.clone(),
            );
        }
        if !self.per_image_descriptions.is_empty() {
            let descriptions: Vec<String> = self
                .per_image_descriptions
                .iter()
                .enumerate()
                .filter(|(_, d)| !d.trim().is_empty())
                .map(|(i, d)| format!("第{}张: {}", i + 1, d))
                .collect();
            if !descriptions.is_empty() {
                fields.insert(
                    "per_image_descriptions".to_string(),
                    descriptions.join("\n"),
                );
            }
        }
        fields.insert(
            "vision_success_count".to_string(),
            self.success_count.to_string(),
        );
        fields.insert(
            "vision_failure_count".to_string(),
            self.failure_count.to_string(),
        );
        fields.insert("vision_source".to_string(), self.source.clone());
        fields.insert(
            "vision_error".to_string(),
            self.error.clone().unwrap_or_default(),
        );
        fields.insert(
            "vision_available".to_string(),
            self.has_usable_description().to_string(),
        );
        fields.insert(
            "sticker_count".to_string(),
            self.sticker_count().to_string(),
        );
        fields
    }

    pub fn sticker_count(&self) -> usize {
        self.sticker_flags.iter().filter(|f| **f).count()
    }
}

/// VLM 视觉客户端 — 图片理解与情绪分类
pub struct VisionClient<A: AIClient, L: PromptTemplateLoader> {
    config: Arc<XueliConfig>,
    client: Arc<A>,
    template_loader: Arc<L>,
    locale: String,
    invocation_router: Option<Arc<ModelInvocationRouter>>,
}

impl<A: AIClient + 'static, L: PromptTemplateLoader> VisionClient<A, L> {
    pub fn new(
        config: Arc<XueliConfig>,
        client: Arc<A>,
        template_loader: Arc<L>,
        locale: impl Into<String>,
    ) -> Self {
        Self {
            config,
            client,
            template_loader,
            locale: locale.into(),
            invocation_router: None,
        }
    }

    pub fn with_invocation_router(mut self, router: Arc<ModelInvocationRouter>) -> Self {
        self.invocation_router = Some(router);
        self
    }

    pub fn is_available(&self) -> bool {
        self.config.is_vision_service_configured()
    }

    /// 检查视觉服务是否启用
    pub fn enabled(&self) -> bool {
        self.is_configured()
    }

    /// 检查视觉服务是否已配置
    pub fn is_configured(&self) -> bool {
        self.config.is_vision_service_configured()
    }

    /// 获取视觉服务状态描述
    pub fn status(&self) -> String {
        self.config.vision_service_status().to_string()
    }

    async fn load_template(&self, name: &str, fallback: &str) -> String {
        self.template_loader
            .get_template(&self.locale, name)
            .await
            .unwrap_or_else(|_| fallback.to_string())
    }

    async fn load_and_render(
        &self,
        name: &str,
        variables: &[(&str, &str)],
        fallback: &str,
    ) -> String {
        let template = self.load_template(name, fallback).await;
        let vars: HashMap<&str, &str> = variables.iter().copied().collect();
        self.template_loader.render(&template, &vars)
    }

    async fn build_system_prompt(&self) -> String {
        self.load_template("vision", FALLBACK_VISION_SYSTEM_PROMPT)
            .await
    }

    async fn build_user_text(&self, user_text: &str, image_count: usize) -> String {
        let clean_text = user_text.trim();
        let image_count_line = format!("图片数量={}", image_count);
        let user_text_line = if clean_text.is_empty() {
            "用户原话为空".to_string()
        } else {
            format!("用户原话={}", clean_text)
        };
        self.load_and_render(
            "vision_user_prompt",
            &[
                ("image_count_line", &image_count_line),
                ("user_text_line", &user_text_line),
            ],
            FALLBACK_VISION_USER_PROMPT,
        )
        .await
    }

    async fn build_emotion_system_prompt(&self, emotion_labels: &str, reply_tones: &str) -> String {
        self.load_and_render(
            "vision_emotion",
            &[
                ("emotion_labels", emotion_labels),
                ("reply_tones", reply_tones),
            ],
            FALLBACK_VISION_EMOTION_PROMPT,
        )
        .await
    }

    async fn build_sticker_prompt(&self) -> String {
        self.load_template("vision_sticker_prompt", FALLBACK_VISION_STICKER_PROMPT)
            .await
    }

    fn get_vision_model(&self) -> XueliResult<String> {
        self.config
            .model
            .vision_model
            .as_ref()
            .cloned()
            .ok_or_else(|| "未配置 VLM 模型".into())
    }

    fn build_image_url(image_base64: &str, mime_type: &str) -> String {
        if image_base64.starts_with("data:") {
            image_base64.to_string()
        } else {
            format!("data:{};base64,{}", mime_type, image_base64)
        }
    }

    pub async fn analyze_image(
        &self,
        image_base64: &str,
        prompt: &str,
        mime_type: &str,
    ) -> XueliResult<String> {
        let model = self.get_vision_model()?;
        let image_url = Self::build_image_url(image_base64, mime_type);

        let messages = vec![
            ChatMessage::text("system", "你是一个视觉分析助手。请仔细描述图片内容。"),
            ChatMessage::multimodal("user", prompt, &[image_url], mime_type),
        ];

        let request = ChatCompletionRequest {
            model,
            messages,
            temperature: Some(0.3),
            max_tokens: Some(1024),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = self.client.chat_completion(&request).await?;
        Ok(response.content)
    }

    pub async fn analyze_images(
        &self,
        image_base64_list: &[String],
        user_text: &str,
        _is_group: bool,
        trace_id: &str,
        _session_key: &str,
        _message_id: &str,
    ) -> XueliResult<ImageAnalysisResult> {
        let image_count = image_base64_list.len();
        if image_count == 0 {
            return Ok(ImageAnalysisResult {
                source: "vision".to_string(),
                ..ImageAnalysisResult::empty()
            });
        }

        if !self.is_available() {
            return Ok(ImageAnalysisResult {
                success_count: 0,
                failure_count: image_count,
                source: self.config.vision_service_status().to_string(),
                error: Some("视觉服务不可用".to_string()),
                ..ImageAnalysisResult::empty()
            });
        }

        let model = self.get_vision_model()?;
        let image_urls: Vec<String> = image_base64_list
            .iter()
            .map(|data| Self::build_image_url(data, "image/jpeg"))
            .collect();

        let system_prompt = self.build_system_prompt().await;
        let user_prompt = self.build_user_text(user_text, image_count).await;

        let messages = vec![
            ChatMessage::text("system", system_prompt),
            ChatMessage::multimodal("user", &user_prompt, &image_urls, "image/jpeg"),
        ];

        let response_content = if let Some(ref router) = self.invocation_router {
            let client = Arc::clone(&self.client);
            let model = model.clone();
            router
                .submit(
                    &InvocationTask::ImageAnalysis,
                    move || {
                        let client = Arc::clone(&client);
                        let model = model.clone();
                        let messages = messages.clone();
                        async move {
                            let request = ChatCompletionRequest {
                                model,
                                messages,
                                temperature: Some(0.1),
                                max_tokens: Some(2048),
                                stream: false,
                                tools: None,
                                tool_choice: None,
                                extra_params: Default::default(),
                            };
                            let response = client.chat_completion(&request).await?;
                            Ok(response.content)
                        }
                    },
                    trace_id,
                    None,
                )
                .await?
        } else {
            let request = ChatCompletionRequest {
                model,
                messages,
                temperature: Some(0.1),
                max_tokens: Some(2048),
                stream: false,
                tools: None,
                tool_choice: None,
                extra_params: Default::default(),
            };
            let response = self.client.chat_completion(&request).await?;
            response.content
        };

        Ok(parse_vision_response(&response_content, image_count))
    }

    pub async fn classify_sticker_emotion(
        &self,
        image_base64: &str,
        emotion_labels: &[String],
        reply_tones: &[String],
        trace_id: &str,
        _session_key: &str,
        _message_id: &str,
    ) -> XueliResult<StickerEmotionResult> {
        if !self.is_available() {
            return Err("视觉服务不可用".into());
        }

        let model = self.get_vision_model()?;

        let labels: Vec<String> = emotion_labels
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let labels_ref: Vec<&str> = if labels.is_empty() {
            DEFAULT_EMOTION_LABELS.to_vec()
        } else {
            labels.iter().map(|s| s.as_str()).collect()
        };
        let label_str = labels_ref.join(" / ");

        let tones: Vec<String> = if reply_tones.is_empty() {
            DEFAULT_REPLY_TONES.iter().map(|s| s.to_string()).collect()
        } else {
            reply_tones
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let tones_str: Vec<&str> = tones.iter().map(|s| s.as_str()).collect();
        let tone_str = tones_str.join(" / ");

        let emotion_system_prompt = self
            .build_emotion_system_prompt(&label_str, &tone_str)
            .await;
        let sticker_prompt = self.build_sticker_prompt().await;

        let image_url = if image_base64.starts_with("data:") {
            image_base64.to_string()
        } else {
            format!("data:image/gif;base64,{}", image_base64)
        };

        let messages = vec![
            ChatMessage::text("system", emotion_system_prompt),
            ChatMessage::multimodal("user", &sticker_prompt, &[image_url], "image/gif"),
        ];

        let response_content = if let Some(ref router) = self.invocation_router {
            let client = Arc::clone(&self.client);
            let model = model.clone();
            router
                .submit(
                    &InvocationTask::ImageAnalysis,
                    move || {
                        let client = Arc::clone(&client);
                        let model = model.clone();
                        let messages = messages.clone();
                        async move {
                            let request = ChatCompletionRequest {
                                model,
                                messages,
                                temperature: Some(0.1),
                                max_tokens: Some(512),
                                stream: false,
                                tools: None,
                                tool_choice: None,
                                extra_params: Default::default(),
                            };
                            let response = client.chat_completion(&request).await?;
                            Ok(response.content)
                        }
                    },
                    trace_id,
                    None,
                )
                .await?
        } else {
            let request = ChatCompletionRequest {
                model,
                messages,
                temperature: Some(0.1),
                max_tokens: Some(512),
                stream: false,
                tools: None,
                tool_choice: None,
                extra_params: Default::default(),
            };
            let response = self.client.chat_completion(&request).await?;
            response.content
        };

        let data = extract_json_object(&response_content);
        Ok(parse_sticker_emotion_result(&data, &labels_ref, &tones_str))
    }
}

fn extract_json_object(content: &str) -> serde_json::Value {
    let text = content.trim();
    if text.is_empty() {
        return serde_json::Value::Object(serde_json::Map::new());
    }

    if let Ok(data) = serde_json::from_str::<serde_json::Value>(text) {
        if data.is_object() {
            return data;
        }
    }

    if let Some(data) = extract_fenced_json(text) {
        return data;
    }

    if let Some(data) = extract_braced_json(text) {
        return data;
    }

    serde_json::Value::Object(serde_json::Map::new())
}

fn extract_fenced_json(text: &str) -> Option<serde_json::Value> {
    let re = regex::Regex::new(r"```(?:json)?\s*(\{(?:[^{}]|\{[^{}]*\})*\})\s*```").ok()?;
    let caps = re.captures(text)?;
    let inner = caps.get(1)?.as_str();
    serde_json::from_str::<serde_json::Value>(inner)
        .ok()
        .filter(|v| v.is_object())
}

fn extract_braced_json(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut end_idx = None;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                end_idx = Some(i);
                break;
            }
        }
    }
    let end = end_idx?;
    let slice = &text[start..=end];
    serde_json::from_str::<serde_json::Value>(slice)
        .ok()
        .filter(|v| v.is_object())
}

fn float_value(value: &serde_json::Value, default: f64) -> f64 {
    value.as_f64().unwrap_or(default)
}

fn str_value(value: &serde_json::Value) -> String {
    value.as_str().unwrap_or("").trim().to_string()
}

fn parse_vision_response(content: &str, image_count: usize) -> ImageAnalysisResult {
    let data = extract_json_object(content);
    let mut descriptions: Vec<String> = Vec::new();
    let mut sticker_flags: Vec<bool> = Vec::new();
    let mut sticker_confidences: Vec<f64> = Vec::new();
    let mut sticker_reasons: Vec<String> = Vec::new();

    if let Some(images) = data.get("images").and_then(|v| v.as_array()) {
        for item in images.iter().take(image_count) {
            if item.is_object() {
                descriptions.push(str_value(&item["description"]));
                sticker_flags.push(item["is_sticker"].as_bool().unwrap_or(false));
                sticker_confidences.push(float_value(&item["sticker_confidence"], 0.0));
                sticker_reasons.push(str_value(&item["sticker_reason"]));
            }
        }
    }

    if descriptions.is_empty() {
        if let Some(per_image) = data
            .get("per_image_descriptions")
            .and_then(|v| v.as_array())
        {
            descriptions = per_image
                .iter()
                .take(image_count)
                .filter_map(|v| {
                    let s = str_value(v);
                    if s.is_empty() {
                        None
                    } else {
                        Some(s)
                    }
                })
                .collect();
        }
    }

    let mut merged = str_value(&data["merged_description"]);

    if descriptions.is_empty() && !content.trim().is_empty() {
        let fallback = content.trim().to_string();
        if image_count == 1 {
            descriptions = vec![format!("第1张: {}", fallback)];
        }
        merged = fallback;
    }

    if merged.is_empty() && !descriptions.is_empty() {
        let parts: Vec<&str> = descriptions
            .iter()
            .filter_map(|d| {
                let s = d.trim();
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            })
            .collect();
        merged = parts.join("；");
    }

    while descriptions.len() < image_count {
        descriptions.push(String::new());
    }
    while sticker_flags.len() < image_count {
        sticker_flags.push(false);
    }
    while sticker_confidences.len() < image_count {
        sticker_confidences.push(0.0);
    }
    while sticker_reasons.len() < image_count {
        sticker_reasons.push(String::new());
    }

    let success_count = descriptions
        .iter()
        .filter(|d| !d.trim().is_empty())
        .count()
        .min(image_count);
    let failure_count = image_count.saturating_sub(success_count);

    ImageAnalysisResult {
        per_image_descriptions: descriptions,
        merged_description: merged,
        sticker_flags,
        sticker_emotion_labels: Vec::new(),
        sticker_confidences,
        sticker_reasons,
        success_count,
        failure_count,
        source: if !content.trim().is_empty() {
            "vision".to_string()
        } else {
            "vision_error".to_string()
        },
        error: None,
    }
}

fn parse_sticker_emotion_result(
    data: &serde_json::Value,
    valid_labels: &[&str],
    valid_tones: &[&str],
) -> StickerEmotionResult {
    let all_emotions_raw: Vec<String> = data
        .get("all_emotions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let s = str_value(v);
                    if s.is_empty() {
                        None
                    } else {
                        Some(s)
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let all_emotions: Vec<String> = all_emotions_raw
        .into_iter()
        .filter(|e| valid_labels.contains(&e.as_str()))
        .take(3)
        .collect();

    let primary = str_value(&data["primary_emotion"]);
    let primary = if !primary.is_empty() {
        primary
    } else if !all_emotions.is_empty() {
        all_emotions[0].clone()
    } else {
        valid_labels
            .first()
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    let mut all_emotions = all_emotions;
    if !primary.is_empty()
        && !valid_labels.contains(&primary.as_str())
        && !all_emotions.contains(&primary)
    {
        all_emotions.insert(0, primary.clone());
    }

    let reply_tones: Vec<String> = data
        .get("reply_tones")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let s = str_value(v);
                    if valid_tones.contains(&s.as_str()) {
                        Some(s)
                    } else {
                        None
                    }
                })
                .take(3)
                .collect()
        })
        .unwrap_or_default();

    let reply_intents_raw: Vec<String> = data
        .get("reply_intents")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let s = str_value(v);
                    if s.is_empty() || !s.contains('-') {
                        None
                    } else {
                        Some(s)
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let mut reply_intents: Vec<String> = reply_intents_raw
        .into_iter()
        .filter(|item| {
            if let Some((tone, emotion)) = item.split_once('-') {
                valid_tones.contains(&tone) && valid_labels.contains(&emotion)
            } else {
                false
            }
        })
        .take(3)
        .collect();

    if !reply_tones.is_empty() && !primary.is_empty() {
        let derived = format!("{}-{}", reply_tones[0], primary);
        if !reply_intents.contains(&derived) {
            reply_intents.insert(0, derived);
        }
    }
    reply_intents.truncate(3);

    StickerEmotionResult {
        primary_emotion: primary,
        all_emotions,
        reply_tones,
        reply_intents,
        confidence: float_value(&data["confidence"], 0.0),
        reason: str_value(&data["reason"]),
        intensity: float_value(&data["intensity"], 0.5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sticker_emotion_result_parse_failed() {
        let r = StickerEmotionResult::parse_failed();
        assert_eq!(r.primary_emotion, "neutral");
        assert_eq!(r.confidence, 0.0);
        assert_eq!(r.intensity, 0.0);
        assert_eq!(r.reason, "parse_failed");
        assert!(r.all_emotions.is_empty());
    }

    #[test]
    fn test_image_analysis_result_empty() {
        let r = ImageAnalysisResult::empty();
        assert!(r.per_image_descriptions.is_empty());
        assert!(r.merged_description.is_empty());
        assert!(!r.has_usable_description());
    }

    #[test]
    fn test_image_analysis_result_to_prompt_fields_merged() {
        let mut r = ImageAnalysisResult::empty();
        r.merged_description = "合并描述".to_string();
        let fields = r.to_prompt_fields();
        assert_eq!(
            fields.get("merged_description"),
            Some(&"合并描述".to_string())
        );
    }

    #[test]
    fn test_image_analysis_result_to_prompt_fields_per_image() {
        let mut r = ImageAnalysisResult::empty();
        r.per_image_descriptions = vec!["猫".to_string(), "狗".to_string()];
        let fields = r.to_prompt_fields();
        let desc = fields.get("per_image_descriptions").unwrap();
        assert!(desc.contains("第1张: 猫"));
        assert!(desc.contains("第2张: 狗"));
    }

    #[test]
    fn test_image_analysis_result_merged_takes_priority() {
        let mut r = ImageAnalysisResult::empty();
        r.merged_description = "合并".to_string();
        r.per_image_descriptions = vec!["猫".to_string()];
        let fields = r.to_prompt_fields();
        assert_eq!(fields.get("merged_description"), Some(&"合并".to_string()));
        assert_eq!(
            fields.get("per_image_descriptions"),
            Some(&"第1张: 猫".to_string())
        );
    }

    #[test]
    fn test_has_usable_description() {
        let mut r = ImageAnalysisResult::empty();
        assert!(!r.has_usable_description());
        r.merged_description = "  ".to_string();
        assert!(!r.has_usable_description());
        r.merged_description = "内容".to_string();
        assert!(r.has_usable_description());
    }

    #[test]
    fn test_extract_json_object_direct() {
        let json = r#"{"primary_emotion":"开心","confidence":0.9}"#;
        let result = extract_json_object(json);
        assert_eq!(result["primary_emotion"].as_str().unwrap(), "开心");
        assert!((result["confidence"].as_f64().unwrap() - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_extract_json_object_fenced() {
        let text = "以下是结果\n```json\n{\"primary_emotion\":\"悲伤\"}\n```\n以上";
        let result = extract_json_object(text);
        assert_eq!(result["primary_emotion"].as_str().unwrap(), "悲伤");
    }

    #[test]
    fn test_extract_json_object_braced() {
        let text = "前缀文字 {\"key\": \"value\"} 后缀文字";
        let result = extract_json_object(text);
        assert_eq!(result["key"].as_str().unwrap(), "value");
    }

    #[test]
    fn test_extract_json_object_empty() {
        let result = extract_json_object("");
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_parse_vision_response_images_array() {
        let json = r#"{"images":[{"description":"猫","is_sticker":true,"sticker_confidence":0.8,"sticker_reason":"表情包风格"}],"merged_description":"猫图"}"#;
        let result = parse_vision_response(json, 1);
        assert_eq!(result.per_image_descriptions.len(), 1);
        assert_eq!(result.per_image_descriptions[0], "猫");
        assert!(result.sticker_flags[0]);
        assert!((result.sticker_confidences[0] - 0.8).abs() < 0.001);
        assert_eq!(result.sticker_reasons[0], "表情包风格");
        assert_eq!(result.merged_description, "猫图");
        assert_eq!(result.success_count, 1);
    }

    #[test]
    fn test_parse_vision_response_plain_text() {
        let result = parse_vision_response("这是一张风景图", 1);
        assert_eq!(result.merged_description, "这是一张风景图");
        assert!(!result.sticker_flags.is_empty());
        assert!(!result.sticker_flags[0]);
    }

    #[test]
    fn test_parse_vision_response_pads_arrays() {
        let result = parse_vision_response(r#"{"merged_description":"test"}"#, 3);
        assert_eq!(result.per_image_descriptions.len(), 3);
        assert_eq!(result.sticker_flags.len(), 3);
        assert_eq!(result.sticker_confidences.len(), 3);
        assert_eq!(result.sticker_reasons.len(), 3);
    }

    #[test]
    fn test_parse_sticker_emotion_result_basic() {
        let json = serde_json::json!({
            "primary_emotion": "开心",
            "confidence": 0.9,
            "all_emotions": ["开心", "喜欢"],
            "reply_tones": ["轻松"],
            "reply_intents": ["轻松-开心"],
            "reason": "表情包呈现愉悦感",
            "intensity": 0.7
        });
        let labels: &[&str] = &["开心", "悲伤", "喜欢"];
        let tones: &[&str] = &["轻松", "温柔"];
        let result = parse_sticker_emotion_result(&json, labels, tones);
        assert_eq!(result.primary_emotion, "开心");
        assert!((result.confidence - 0.9).abs() < 0.001);
        assert_eq!(result.all_emotions.len(), 2);
        assert_eq!(result.reply_tones.len(), 1);
        assert_eq!(result.reply_intents.len(), 1);
        assert!((result.intensity - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_parse_sticker_emotion_result_defaults() {
        let data = serde_json::Value::Object(serde_json::Map::new());
        let labels: &[&str] = &["开心", "悲伤"];
        let tones: &[&str] = &["轻松"];
        let result = parse_sticker_emotion_result(&data, labels, tones);
        assert_eq!(result.primary_emotion, "开心");
        assert!((result.confidence - 0.0).abs() < 0.001);
        assert!((result.intensity - 0.5).abs() < 0.001);
    }
}
