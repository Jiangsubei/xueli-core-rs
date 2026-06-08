use std::sync::Arc;

use base64::Engine;

use crate::core::platform_types::InboundEvent;
use crate::emoji::manager::EmojiManager;
use crate::prelude::{AIClient, PromptTemplateLoader, XueliResult};
use crate::services::image_client::ImageClient;
use crate::services::vision_client::{ImageAnalysisResult, StickerEmotionResult, VisionClient};

pub struct ImagePipeline<A: AIClient + 'static, L: PromptTemplateLoader + 'static> {
    image_client: Arc<ImageClient>,
    vision_client: Arc<VisionClient<A, L>>,
    emoji_manager: Option<Arc<EmojiManager<A, L>>>,
    enabled: bool,
}

impl<A: AIClient + 'static, L: PromptTemplateLoader + 'static> ImagePipeline<A, L> {
    pub fn new(
        vision_client: Arc<VisionClient<A, L>>,
        image_client: Arc<ImageClient>,
        emoji_manager: Option<Arc<EmojiManager<A, L>>>,
    ) -> Self {
        let enabled = vision_client.is_available();
        Self {
            image_client,
            vision_client,
            emoji_manager,
            enabled,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub async fn analyze_image_url(&self, url: &str, prompt: &str) -> XueliResult<String> {
        let base64 = self.image_client.download_as_base64(url).await?;
        self.vision_client
            .analyze_image(&base64, prompt, "image/jpeg")
            .await
    }

    pub async fn download_split(
        &self,
        event: &InboundEvent,
    ) -> XueliResult<(Vec<String>, Vec<(Vec<u8>, String)>)> {
        let mut normal_images: Vec<String> = Vec::new();
        let mut sticker_bytes: Vec<(Vec<u8>, String)> = Vec::new();

        let image_urls = Self::extract_image_urls(event);
        if image_urls.is_empty() {
            return Ok((normal_images, sticker_bytes));
        }

        let sticker_urls = Self::extract_sticker_urls(event);

        for url in &image_urls {
            let base64 = match self.image_client.download_as_base64(url).await {
                Ok(b) => b,
                Err(_) => continue,
            };
            if sticker_urls.contains(url) {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&base64) {
                    sticker_bytes.push((bytes, url.clone()));
                }
            } else {
                normal_images.push(base64);
            }
        }

        Ok((normal_images, sticker_bytes))
    }

    pub async fn analyze(
        &self,
        event: &InboundEvent,
        user_text: &str,
        is_group: bool,
    ) -> XueliResult<ImageAnalysisResult> {
        if !self.enabled {
            return Ok(ImageAnalysisResult::empty());
        }

        let (normal_images, sticker_data) = self.download_split(event).await?;

        let total_image_count = Self::extract_image_urls(event).len();
        if normal_images.is_empty() && sticker_data.is_empty() {
            return Ok(ImageAnalysisResult {
                success_count: 0,
                failure_count: total_image_count,
                source: "image_download_error".to_string(),
                error: Some("image download failed".to_string()),
                ..ImageAnalysisResult::empty()
            });
        }

        let mut result = if !normal_images.is_empty() {
            self.vision_client
                .analyze_images(&normal_images, user_text, is_group)
                .await?
        } else {
            ImageAnalysisResult::empty()
        };

        if !sticker_data.is_empty() {
            for (data, _file_id) in &sticker_data {
                let b64 = base64::engine::general_purpose::STANDARD.encode(data);
                let emotion_labels: Vec<String> = Vec::new();
                let reply_tones: Vec<String> = Vec::new();
                match self
                    .vision_client
                    .classify_sticker_emotion(&b64, &emotion_labels, &reply_tones)
                    .await
                {
                    Ok(emotion) => {
                        result.sticker_flags.push(true);
                        result
                            .sticker_emotion_labels
                            .push(emotion.primary_emotion.clone());
                        result.sticker_confidences.push(emotion.confidence);
                        result.sticker_reasons.push(emotion.reason.clone());
                    }
                    Err(_) => {
                        result.sticker_flags.push(false);
                        result.sticker_emotion_labels.push(String::new());
                        result.sticker_confidences.push(0.0);
                        result.sticker_reasons.push(String::new());
                    }
                }

                if let Some(ref em) = self.emoji_manager {
                    let _ = em.capture_sticker(data, "", "", "").await;
                }
            }
        }

        Ok(result)
    }

    pub async fn process_detection_result(
        &self,
        sticker_data: &[(Vec<u8>, String)],
        emotion_results: &[StickerEmotionResult],
    ) -> XueliResult<()> {
        let em = match &self.emoji_manager {
            Some(e) => e,
            None => return Ok(()),
        };

        for ((data, _file_id), _emotion) in sticker_data.iter().zip(emotion_results.iter()) {
            let _ = em.capture_sticker(data, "", "", "").await;
        }

        Ok(())
    }

    fn extract_image_urls(event: &InboundEvent) -> Vec<String> {
        Self::extract_urls_by_segment_type(event, |seg| {
            let is_image = seg
                .get("type")
                .and_then(|v| v.as_str())
                .map(|t| t.eq_ignore_ascii_case("image"))
                .unwrap_or(false);
            is_image
        })
    }

    fn extract_sticker_urls(event: &InboundEvent) -> Vec<String> {
        Self::extract_urls_by_segment_type(event, |seg| {
            let is_image = seg
                .get("type")
                .and_then(|v| v.as_str())
                .map(|t| t.eq_ignore_ascii_case("image"))
                .unwrap_or(false);
            let is_sticker = seg
                .get("data")
                .and_then(|d| d.get("classified_kind"))
                .and_then(|c| c.as_str())
                .map(|c| c == "sticker")
                .unwrap_or(false);
            is_image && is_sticker
        })
    }

    fn extract_urls_by_segment_type(
        event: &InboundEvent,
        predicate: impl Fn(&serde_json::Value) -> bool,
    ) -> Vec<String> {
        let raw = match &event.raw_payload {
            Some(p) => p,
            None => return Vec::new(),
        };
        let parsed: serde_json::Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let segments = match parsed.get("segments").and_then(|v| v.as_array()) {
            Some(s) => s,
            None => return Vec::new(),
        };
        segments
            .iter()
            .filter(|seg| predicate(seg))
            .filter_map(|seg| {
                seg.get("data")
                    .and_then(|d| d.get("url"))
                    .and_then(|u| u.as_str())
                    .map(|u| u.to_string())
            })
            .collect()
    }
}
