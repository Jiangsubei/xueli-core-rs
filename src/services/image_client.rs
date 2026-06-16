use crate::prelude::XueliResult;
use regex::Regex;
use std::sync::LazyLock;

/// 完整的 HTML 实体解码：命名实体 + 十六进制/十进制数字字符引用。
fn decode_html_entities(input: &str) -> String {
    static ENTITY_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"&(?:#(\d+)|#x([0-9a-fA-F]+)|(amp|lt|gt|quot|apos|nbsp));").unwrap()
    });

    ENTITY_RE
        .replace_all(input, |caps: &regex::Captures<'_>| {
            if let Some(dec) = caps.get(1) {
                // 十进制数字字符引用: &#...;
                if let Ok(n) = dec.as_str().parse::<u32>() {
                    if let Some(c) = char::from_u32(n) {
                        return c.to_string();
                    }
                }
            } else if let Some(hex) = caps.get(2) {
                // 十六进制数字字符引用: &#x...;
                if let Ok(n) = u32::from_str_radix(hex.as_str(), 16) {
                    if let Some(c) = char::from_u32(n) {
                        return c.to_string();
                    }
                }
            } else if let Some(name) = caps.get(3) {
                // 命名实体
                match name.as_str() {
                    "amp" => return "&".to_string(),
                    "lt" => return "<".to_string(),
                    "gt" => return ">".to_string(),
                    "quot" => return "\"".to_string(),
                    "apos" => return "'".to_string(),
                    "nbsp" => return " ".to_string(),
                    _ => {}
                }
            }
            // 无法识别的实体，保留原文
            caps.get(0).unwrap().as_str().to_string()
        })
        .into_owned()
}

pub struct ImageClient {
    client: reqwest::Client,
    base_url: String,
}

impl ImageClient {
    pub fn new() -> XueliResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;
        Ok(Self {
            client,
            base_url: String::new(),
        })
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn fix_url(url: &str) -> String {
        decode_html_entities(url)
    }

    pub async fn download(&self, url: &str) -> XueliResult<Vec<u8>> {
        let fixed_url = Self::fix_url(url);
        let response = self
            .client
            .get(&fixed_url)
            .send()
            .await
            .map_err(|e| format!("图片下载失败: {}", e))?;

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("读取图片数据失败: {}", e).into())
    }

    pub async fn download_as_base64(&self, url: &str) -> XueliResult<String> {
        use base64::Engine;
        let bytes = self.download(url).await?;
        Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
    }

    pub fn download_image_from_base64(base64_str: &str) -> XueliResult<Vec<u8>> {
        use base64::Engine;
        let cleaned = if let Some(idx) = base64_str.find(";base64,") {
            &base64_str[idx + 8..]
        } else if base64_str.contains(':') && base64_str.contains(',') {
            if let Some(idx) = base64_str.find(',') {
                &base64_str[idx + 1..]
            } else {
                base64_str
            }
        } else {
            base64_str
        };
        base64::engine::general_purpose::STANDARD
            .decode(cleaned)
            .map_err(|e| format!("Base64 解码失败: {}", e).into())
    }

    pub async fn process_image_segment(&self, segment: &serde_json::Value) -> XueliResult<Vec<u8>> {
        if let Some(url) = segment.get("url").and_then(|v| v.as_str()) {
            self.download(url).await
        } else if let Some(data) = segment
            .get("data")
            .or_else(|| segment.get("base64"))
            .and_then(|v| v.as_str())
        {
            Self::download_image_from_base64(data)
        } else {
            Err("图片消息段缺少 url/data/base64 字段".into())
        }
    }

    pub async fn get_mface_image_url(&self, key: &str) -> XueliResult<String> {
        let url = format!("{}/get_image", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({"file": key});
        let response = self
            .client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("获取 mface 图片 URL 失败: {}", e))?;
        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("解析响应失败: {}", e))?;
        let image_url = data
            .get("data")
            .and_then(|d| d.get("url"))
            .and_then(|v| v.as_str());
        match image_url {
            Some(u) => Ok(Self::fix_url(u)),
            None => Err("响应中未找到图片 URL".into()),
        }
    }
}

impl Default for ImageClient {
    fn default() -> Self {
        Self::new().expect("ImageClient 初始化失败")
    }
}
