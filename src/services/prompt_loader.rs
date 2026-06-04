use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;

use crate::prelude::XueliResult;
use crate::traits::prompt_template::{PromptTemplateLoader, PromptTemplateMap};

/// 基于文件系统的提示词模板加载器
///
/// 从 `prompts/{locale}/` 目录加载 `.prompt` 文件
pub struct FilePromptTemplateLoader {
    base_dir: PathBuf,
    cache: HashMap<String, PromptTemplateMap>,
}

impl FilePromptTemplateLoader {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            cache: HashMap::new(),
        }
    }
}

impl PromptTemplateLoader for FilePromptTemplateLoader {
    fn load_templates(
        &self,
        locale: &str,
    ) -> impl Future<Output = XueliResult<PromptTemplateMap>> + Send {
        let base_dir = self.base_dir.clone();
        let locale = locale.to_string();
        async move {
            let dir = base_dir.join(&locale);
            if !dir.exists() {
                return Err(format!("模板目录不存在: {:?}", dir).into());
            }

            let mut map = HashMap::new();
            let mut entries = tokio::fs::read_dir(&dir)
                .await
                .map_err(|e| format!("无法读取模板目录 {:?}: {}", dir, e))?;

            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| format!("无法读取目录条目: {}", e))?
            {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("prompt") {
                    let name = path
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();
                    let content = tokio::fs::read_to_string(&path)
                        .await
                        .map_err(|e| format!("无法读取模板文件 {:?}: {}", path, e))?;
                    map.insert(name, content);
                }
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
        let base_dir = self.base_dir.clone();
        let locale = locale.to_string();
        let name = name.to_string();
        async move {
            // 先检查缓存
            if let Some(cached) = cache.get(&locale) {
                if let Some(template) = cached.get(&name) {
                    return Ok(template.clone());
                }
            }

            // 直接从文件加载
            let path = base_dir.join(&locale).join(format!("{name}.prompt"));
            if path.exists() {
                tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|e| format!("无法读取模板文件 {:?}: {}", path, e).into())
            } else {
                Err(format!("模板不存在: {} / {}", locale, name).into())
            }
        }
    }

    fn render(&self, template: &str, variables: &HashMap<&str, &str>) -> String {
        let mut result = template.to_string();
        for (key, value) in variables {
            result = result.replace(&format!("{{{{{}}}}}", key), value);
        }
        result
    }
}

/// 空模板加载器 — 总是返回默认模板（用于测试或未配置模板的场合）
pub struct NoopPromptTemplateLoader;

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
        async move { Err(format!("模板不存在: {}", name).into()) }
    }
}
