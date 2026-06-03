use crate::services::vision_client::VisionClient;
use crate::services::image_client::ImageClient;
use crate::traits::ai_client::AIClient;

/// 图片管线 — 处理图片消息识别与回复
pub struct ImagePipeline {
    vision_client: VisionClient,
    image_client: ImageClient,
}

impl ImagePipeline {
    pub fn new(ai_client: std::sync::Arc<dyn AIClient>, vision_model: String) -> Self {
        use crate::core::config::ModelConfig;
        let config = std::sync::Arc::new(ModelConfig {
            primary_model: String::new(),
            light_model: String::new(),
            vision_model: Some(vision_model),
            api_base: String::new(),
            api_key: String::new(),
            temperature: 0.7,
            max_tokens: 4096,
        });
        Self {
            vision_client: VisionClient::new(config, ai_client),
            image_client: ImageClient::default(),
        }
    }

    /// 分析图片 URL
    pub async fn analyze_image_url(&self, url: &str, prompt: &str) -> Result<String, String> {
        let base64 = self.image_client.download_as_base64(url).await?;
        self.vision_client
            .analyze_image(&base64, prompt, "image/jpeg")
            .await
    }
}