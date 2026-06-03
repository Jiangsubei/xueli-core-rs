/// 图片下载与编码服务
pub struct ImageClient {
    client: reqwest::Client,
}

impl ImageClient {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;
        Ok(Self { client })
    }

    /// 下载图片并返回字节
    pub async fn download(&self, url: &str) -> Result<Vec<u8>, String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("图片下载失败: {}", e))?;

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("读取图片数据失败: {}", e))
    }

    /// 下载图片并编码为 base64
    pub async fn download_as_base64(&self, url: &str) -> Result<String, String> {
        use base64::Engine;
        let bytes = self.download(url).await?;
        Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
    }
}

impl Default for ImageClient {
    fn default() -> Self {
        Self::new().expect("ImageClient 初始化失败")
    }
}