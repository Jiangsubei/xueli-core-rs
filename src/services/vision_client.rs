use std::sync::Arc;

use crate::core::config::XueliConfig;
use crate::prelude::XueliResult;
use crate::traits::ai_client::{AIClient, ChatCompletionRequest, ChatMessage};

/// VLM 视觉客户端 — 图片理解与情绪分类
///
/// 对应 Python 版 `xueli/src/services/vision_client.py`
pub struct VisionClient<A: AIClient> {
    config: Arc<XueliConfig>,
    client: Arc<A>,
}

/// 图片分析结果
#[derive(Debug, Clone)]
pub struct ImageAnalysisResult {
    /// 逐图描述 (per-image descriptions)
    pub per_image_descriptions: Vec<String>,
    /// 合并描述
    pub merged_description: String,
    /// 是否为贴纸（逐张图）
    pub sticker_flags: Vec<bool>,
    /// 贴纸情绪标签
    pub sticker_emotion_labels: Vec<String>,
    /// 识别成功的图片数
    pub success_count: usize,
    /// 识别失败的图片数
    pub failure_count: usize,
}

impl ImageAnalysisResult {
    pub fn empty() -> Self {
        Self {
            per_image_descriptions: Vec::new(),
            merged_description: String::new(),
            sticker_flags: Vec::new(),
            sticker_emotion_labels: Vec::new(),
            success_count: 0,
            failure_count: 0,
        }
    }
}

impl<A: AIClient> VisionClient<A> {
    pub fn new(config: Arc<XueliConfig>, client: Arc<A>) -> Self {
        Self { config, client }
    }

    /// 检查 VLM 是否已配置且可用
    pub fn is_available(&self) -> bool {
        self.config.model.vision_model.is_some()
    }

    /// 分析单张图片
    pub async fn analyze_image(
        &self,
        image_base64: &str,
        prompt: &str,
        mime_type: &str,
    ) -> XueliResult<String> {
        let model = self
            .config
            .model
            .vision_model
            .as_ref()
            .ok_or("未配置 VLM 模型")?
            .clone();

        // 构建多模态消息：纯文本 system prompt + 带图片的 user 消息
        let image_url = if image_base64.starts_with("data:") {
            image_base64.to_string()
        } else {
            format!("data:{};base64,{}", mime_type, image_base64)
        };

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

    /// 分析多张图片（带合并描述）
    pub async fn analyze_images(
        &self,
        image_base64_list: &[String],
        user_text: &str,
        _is_group: bool,
    ) -> XueliResult<ImageAnalysisResult> {
        let model = self
            .config
            .model
            .vision_model
            .as_ref()
            .ok_or("未配置 VLM 模型")?
            .clone();

        let image_urls: Vec<String> = image_base64_list
            .iter()
            .map(|data| {
                if data.starts_with("data:") {
                    data.clone()
                } else {
                    format!("data:image/jpeg;base64,{}", data)
                }
            })
            .collect();

        let prompt = build_vision_user_prompt(user_text, image_urls.len());

        let messages = vec![
            ChatMessage::text("system", build_vision_system_prompt(image_urls.len())),
            ChatMessage::multimodal("user", prompt, &image_urls, "image/jpeg"),
        ];

        let request = ChatCompletionRequest {
            model,
            messages,
            temperature: Some(0.3),
            max_tokens: Some(2048),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = self.client.chat_completion(&request).await?;
        Ok(parse_vision_response(&response.content))
    }

    /// 对贴纸进行情绪分类
    pub async fn classify_sticker_emotion(
        &self,
        image_base64: &str,
        _sticker_type: &str,
    ) -> XueliResult<String> {
        let model = self
            .config
            .model
            .vision_model
            .as_ref()
            .ok_or("未配置 VLM 模型")?
            .clone();

        let image_url = if image_base64.starts_with("data:") {
            image_base64.to_string()
        } else {
            format!("data:image/gif;base64,{}", image_base64)
        };

        let messages = vec![ChatMessage::multimodal(
            "user",
            "请为这张表情贴纸标注一个简短的情绪标签（如：开心、生气、悲伤、惊讶、喜欢、再见、鼓励等），只输出标签词。",
            &[image_url],
            "image/gif",
        )];

        let request = ChatCompletionRequest {
            model,
            messages,
            temperature: Some(0.1),
            max_tokens: Some(50),
            stream: false,
            tools: None,
            tool_choice: None,
            extra_params: Default::default(),
        };

        let response = self.client.chat_completion(&request).await?;
        Ok(response.content.trim().to_string())
    }
}

fn build_vision_system_prompt(image_count: usize) -> String {
    format!(
        "你是一个视觉分析助手。你会收到 {} 张图片。请对每张图片给出独立的描述，然后提供一个整体的合并描述。按 JSON 格式输出。",
        image_count
    )
}

fn build_vision_user_prompt(user_text: &str, image_count: usize) -> String {
    if user_text.is_empty() {
        format!(
            "请分析以下 {} 张图片：\n为每张图片输出 image_{}_description，并给出 merged_description。以 JSON 格式返回。",
            image_count, "{i}"
        )
    } else {
        format!(
            "用户消息：{}\n\n请结合用户消息分析以下 {} 张图片。为每张图片输出 image_{}_description，并给出 merged_description。以 JSON 格式返回。",
            user_text, image_count, "{i}"
        )
    }
}

fn parse_vision_response(content: &str) -> ImageAnalysisResult {
    let mut result = ImageAnalysisResult::empty();
    let text = content.trim();

    // 尝试 JSON 解析
    if let Ok(data) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(merged) = data.get("merged_description").and_then(|v| v.as_str()) {
            result.merged_description = merged.to_string();
        }
        // 提取逐图描述
        let obj = data.as_object();
        if let Some(obj) = obj {
            let mut i = 1;
            loop {
                let key = format!("image_{}_description", i);
                if let Some(desc) = obj.get(&key).and_then(|v| v.as_str()) {
                    result.per_image_descriptions.push(desc.to_string());
                    result.success_count += 1;
                    i += 1;
                } else {
                    break;
                }
            }
        }
        // 提取贴纸标签
        if let Some(stickers) = data.get("sticker_flags").and_then(|v| v.as_array()) {
            for flag in stickers {
                result.sticker_flags.push(flag.as_bool().unwrap_or(false));
            }
        }
        if let Some(labels) = data
            .get("sticker_emotion_labels")
            .and_then(|v| v.as_array())
        {
            for label in labels {
                result
                    .sticker_emotion_labels
                    .push(label.as_str().unwrap_or("").to_string());
            }
        }
    } else {
        // JSON 解析失败时，整个内容作为合并描述
        result.merged_description = text.to_string();
        result.success_count = 0;
        result.failure_count = 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vision_response_valid_json() {
        let json = r#"{"image_1_description": "一只猫", "image_2_description": "一只狗", "merged_description": "猫和狗在一起"}"#;
        let result = parse_vision_response(json);
        assert_eq!(result.per_image_descriptions.len(), 2);
        assert_eq!(result.merged_description, "猫和狗在一起");
        assert_eq!(result.success_count, 2);
    }

    #[test]
    fn test_parse_vision_response_plain_text() {
        let result = parse_vision_response("这是一张风景图");
        assert_eq!(result.merged_description, "这是一张风景图");
        assert_eq!(result.success_count, 0);
    }

    #[test]
    fn test_vision_result_empty() {
        let result = ImageAnalysisResult::empty();
        assert!(result.per_image_descriptions.is_empty());
        assert!(result.merged_description.is_empty());
    }
}
