use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::prelude::XueliResult;
use crate::traits::prompt_template::{PromptTemplateLoader, PromptTemplateMap};

/// 基于文件系统的提示词模板加载器
///
/// 从 `prompts/{locale}/` 目录加载 `.prompt` 文件，支持缓存和运行时重载。
pub struct FilePromptTemplateLoader {
    base_dir: PathBuf,
    /// 缓存：locale -> (模板名 -> 内容)
    cache: Arc<RwLock<HashMap<String, PromptTemplateMap>>>,
}

impl FilePromptTemplateLoader {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 获取模板目录路径
    pub fn template_dir(&self, locale: &str) -> PathBuf {
        self.base_dir.join(locale)
    }

    /// 清除指定 locale 的缓存
    pub async fn invalidate_cache(&self, locale: &str) {
        let mut cache = self.cache.write().await;
        cache.remove(locale);
    }

    /// 清除全部缓存
    pub async fn invalidate_all(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// 克隆内部缓存 Arc（用于跨 async 边界的安全访问）
    pub fn cache_arc(&self) -> Arc<RwLock<HashMap<String, PromptTemplateMap>>> {
        self.cache.clone()
    }

    /// 扫描并返回所有可用的 locale
    pub async fn available_locales(&self) -> XueliResult<Vec<String>> {
        let mut locales = Vec::new();
        if !self.base_dir.exists() {
            return Ok(locales);
        }

        let mut entries = tokio::fs::read_dir(&self.base_dir).await.map_err(|e| {
            crate::core::errors::XueliError::Template(crate::core::errors::TemplateError::Load(
                format!("无法读取模板根目录: {}", e),
            ))
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            crate::core::errors::XueliError::Template(crate::core::errors::TemplateError::Load(
                format!("无法读取目录条目: {}", e),
            ))
        })? {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    locales.push(name.to_string());
                }
            }
        }

        Ok(locales)
    }
}

impl PromptTemplateLoader for FilePromptTemplateLoader {
    fn load_templates(
        &self,
        locale: &str,
    ) -> impl Future<Output = XueliResult<PromptTemplateMap>> + Send {
        let base_dir = self.base_dir.clone();
        let cache = self.cache.clone();
        let locale = locale.to_string();
        async move {
            // 先检查缓存
            {
                let cache_read = cache.read().await;
                if let Some(cached) = cache_read.get(&locale) {
                    return Ok(cached.clone());
                }
            }

            let dir = base_dir.join(&locale);
            if !dir.exists() {
                return Err(crate::core::errors::XueliError::Template(
                    crate::core::errors::TemplateError::NotFound(format!(
                        "模板目录不存在: {:?}",
                        dir
                    )),
                ));
            }

            let mut map = HashMap::new();
            let mut entries = tokio::fs::read_dir(&dir).await.map_err(|e| {
                crate::core::errors::XueliError::Template(crate::core::errors::TemplateError::Load(
                    format!("无法读取模板目录 {:?}: {}", dir, e),
                ))
            })?;

            while let Some(entry) = entries.next_entry().await.map_err(|e| {
                crate::core::errors::XueliError::Template(crate::core::errors::TemplateError::Load(
                    format!("无法读取目录条目: {}", e),
                ))
            })? {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("prompt") {
                    let name = path
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();
                    let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                        crate::core::errors::XueliError::Template(
                            crate::core::errors::TemplateError::Load(format!(
                                "无法读取模板文件 {:?}: {}",
                                path, e
                            )),
                        )
                    })?;
                    map.insert(name, content);
                }
            }

            // 写入缓存
            {
                let mut cache_write = cache.write().await;
                cache_write.insert(locale.clone(), map.clone());
            }

            Ok(map)
        }
    }

    fn get_template(
        &self,
        locale: &str,
        name: &str,
    ) -> impl Future<Output = XueliResult<String>> + Send {
        let cache = self.cache.clone();
        let _base_dir = self.base_dir.clone();
        let locale = locale.to_string();
        let name = name.to_string();
        async move {
            // 先检查缓存
            {
                let cache_read = cache.read().await;
                if let Some(cached) = cache_read.get(&locale) {
                    if let Some(template) = cached.get(&name) {
                        return Ok(template.clone());
                    }
                }
            }

            // 缓存未命中：加载全部模板
            let templates = self.load_templates(&locale).await?;
            templates.get(&name).cloned().ok_or_else(|| {
                crate::core::errors::XueliError::Template(
                    crate::core::errors::TemplateError::NotFound(format!("{} / {}", locale, name)),
                )
            })
        }
    }

    fn render(&self, template: &str, variables: &HashMap<&str, &str>) -> String {
        let mut result = template.to_string();
        for (key, value) in variables {
            result = result.replace(&format!("{{{}}}", key), value);
        }
        result
    }
}

/// 空模板加载器 — 总是返回默认模板（用于测试或未配置模板的场合）
pub struct NoopPromptTemplateLoader;

impl Default for NoopPromptTemplateLoader {
    fn default() -> Self {
        Self
    }
}

impl PromptTemplateLoader for NoopPromptTemplateLoader {
    fn load_templates(
        &self,
        _locale: &str,
    ) -> impl Future<Output = XueliResult<PromptTemplateMap>> + Send {
        async { Ok(HashMap::new()) }
    }

    fn get_template(
        &self,
        _locale: &str,
        name: &str,
    ) -> impl Future<Output = XueliResult<String>> + Send {
        let name = name.to_string();
        async move {
            Err(crate::core::errors::XueliError::Template(
                crate::core::errors::TemplateError::NotFound(name),
            ))
        }
    }
}
