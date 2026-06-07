// 助手身份文本构建器
// 对应 Python 版 xueli/src/handlers/shared/identity_provider.py

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::core::config::XueliConfig;
use crate::traits::prompt_template::PromptTemplateLoader;

/// 提供助手名称/别名和身份提示文本
pub struct IdentityProvider<L: PromptTemplateLoader> {
    config: Arc<XueliConfig>,
    template_loader: Arc<L>,
    locale: String,
    /// 缓存的身份文本
    cached_identity_text: Mutex<Option<String>>,
}

impl<L: PromptTemplateLoader> IdentityProvider<L> {
    pub fn new(
        config: Arc<XueliConfig>,
        template_loader: Arc<L>,
        locale: impl Into<String>,
    ) -> Self {
        Self {
            config,
            template_loader,
            locale: locale.into(),
            cached_identity_text: Mutex::new(None),
        }
    }

    /// 助手名称
    pub fn assistant_name(&self) -> String {
        let name = self.config.identity.name.trim();
        if name.is_empty() {
            "助手".to_string()
        } else {
            name.to_string()
        }
    }

    /// 助手别名
    pub fn assistant_alias(&self) -> String {
        self.config.identity.alias.trim().to_string()
    }

    /// 构建身份文本（带缓存）
    pub async fn build_identity_text(&self) -> String {
        {
            let cached = self.cached_identity_text.lock().await;
            if let Some(ref text) = *cached {
                return text.clone();
            }
        } // drop lock before potential await in render

        let name = self.assistant_name();
        let alias = self.assistant_alias();
        let text = self.render_identity(&name, &alias).await;
        let mut cached = self.cached_identity_text.lock().await;
        *cached = Some(text.clone());
        // 同时设置缓存，供首次调用使用
        text
    }

    /// 构建身份提示词（与 build_identity_text 同义）
    pub async fn build_identity_prompt(&self) -> String {
        self.build_identity_text().await
    }

    /// 暂不清除缓存（当前实现采用无锁缓存，不提供此方法）
    #[allow(dead_code)]
    fn invalidate_cache(&self) {
        // 缓存通过 Mutex 持有，此处仅保留接口占位
    }

    /// 渲染身份文本
    async fn render_identity(&self, name: &str, alias: &str) -> String {
        let aliases_part = if alias.is_empty() {
            String::new()
        } else {
            format!("，别名是\u{201c}{}\u{201d}", alias)
        };
        let aliases_ref = if alias.is_empty() {
            String::new()
        } else {
            format!("或\u{201c}{}\u{201d}", alias)
        };

        // 尝试从模板加载器获取 identity.prompt 并渲染
        let template_result = self
            .template_loader
            .get_template(&self.locale, "identity.prompt")
            .await
            .map(|t| {
                let mut vars = HashMap::new();
                vars.insert("name", name);
                vars.insert("aliases_part", aliases_part.as_str());
                vars.insert("aliases_ref", aliases_ref.as_str());
                self.template_loader.render(&t, &vars)
            });

        match template_result {
            Ok(rendered) => rendered,
            Err(_) => {
                // fallback: 硬编码身份文本
                if !alias.is_empty() {
                    format!(
                        "你的名字是\u{201c}{}\u{201d}，别名是\u{201c}{}\u{201d}。当用户提到\u{201c}{}\u{201d}或\u{201c}{}\u{201d}时，说的都是你。",
                        name, alias, name, alias
                    )
                } else {
                    format!(
                        "你的名字是\u{201c}{}\u{201d}。当用户提到\u{201c}{}\u{201d}时，说的就是你。",
                        name, name
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::{XueliError, XueliResult};
    use crate::traits::prompt_template::PromptTemplateMap;
    use std::collections::HashMap;
    use std::future::Future;

    struct MockTemplateLoader;
    impl PromptTemplateLoader for MockTemplateLoader {
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
                if name == "identity.prompt" {
                    Ok(
                        "你的名字是\"{name}\"{aliases_part}。当用户提到\"{name}\"{aliases_ref}时，说的就是你。"
                            .to_string(),
                    )
                } else {
                    Err(XueliError::Template(
                        crate::core::errors::TemplateError::NotFound("not found".to_string()),
                    ))
                }
            }
        }
    }

    fn test_config() -> Arc<XueliConfig> {
        let mut config = XueliConfig::default();
        config.identity.name = "雪梨".to_string();
        config.identity.alias = "小梨".to_string();
        Arc::new(config)
    }

    #[tokio::test]
    async fn test_identity_text_with_alias() {
        let provider = IdentityProvider::new(test_config(), Arc::new(MockTemplateLoader), "zh-CN");
        let text = provider.build_identity_text().await;
        assert!(text.contains("雪梨"));
        assert!(text.contains("小梨"));
    }

    #[tokio::test]
    async fn test_identity_text_cache() {
        let provider = IdentityProvider::new(test_config(), Arc::new(MockTemplateLoader), "zh-CN");
        let t1 = provider.build_identity_text().await;
        let t2 = provider.build_identity_text().await;
        assert_eq!(t1, t2);
    }

    #[tokio::test]
    async fn test_identity_text_no_alias() {
        let mut config = XueliConfig::default();
        config.identity.name = "助手".to_string();
        config.identity.alias = String::new();
        let provider =
            IdentityProvider::new(Arc::new(config), Arc::new(MockTemplateLoader), "zh-CN");
        let text = provider.build_identity_text().await;
        assert!(text.contains("助手"));
        assert!(!text.contains("\u{201c}\u{201d}")); // 不包含空引号
    }

    /// 测试当模板加载失败时回退到硬编码身份文本
    fn _make_failing_loader() -> impl PromptTemplateLoader {
        struct FailingLoader;
        impl PromptTemplateLoader for FailingLoader {
            fn load_templates(
                &self,
                _locale: &str,
            ) -> impl Future<Output = XueliResult<PromptTemplateMap>> + Send {
                async { Err("fail".into()) }
            }
            fn get_template(
                &self,
                _locale: &str,
                _name: &str,
            ) -> impl Future<Output = XueliResult<String>> + Send {
                async { Err("fail".into()) }
            }
        }
        FailingLoader
    }

    #[tokio::test]
    async fn test_identity_text_fallback_on_template_error() {
        let config = test_config();
        let provider = IdentityProvider::new(config, Arc::new(_make_failing_loader()), "zh-CN");
        let text = provider.build_identity_text().await;
        assert!(text.contains("雪梨"));
        assert!(text.contains("小梨"));
        assert!(text.contains("你的名字是")); // 回退文本包含中文
    }
}
