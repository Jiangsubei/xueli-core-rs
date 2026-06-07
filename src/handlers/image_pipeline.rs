use crate::core::config::XueliConfig;
use crate::prelude::XueliResult;
use crate::services::image_client::ImageClient;
use crate::services::vision_client::VisionClient;
use crate::traits::ai_client::AIClient;

/// 图片管线 — 处理图片消息识别与回复
pub struct ImagePipeline<A: AIClient> {
    vision_client: VisionClient<A>,
    image_client: ImageClient,
}

impl<A: AIClient> ImagePipeline<A> {
    pub fn new(config: std::sync::Arc<XueliConfig>, ai_client: std::sync::Arc<A>) -> Self {
        Self {
            vision_client: VisionClient::new(config, ai_client),
            image_client: ImageClient::default(),
        }
    }

    /// 分析图片 URL
    pub async fn analyze_image_url(&self, url: &str, prompt: &str) -> XueliResult<String> {
        let base64 = self.image_client.download_as_base64(url).await?;
        self.vision_client
            .analyze_image(&base64, prompt, "image/jpeg")
            .await
    }
}
