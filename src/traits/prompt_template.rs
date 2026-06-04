use std::collections::HashMap;
use std::future::Future;

use crate::prelude::XueliResult;

/// 提示词模板内容类型
pub type PromptTemplateMap = HashMap<String, String>;

/// 提示词模板加载器 trait — 支持 i18n 和运行时切换
pub trait PromptTemplateLoader: Send + Sync {
    /// 加载指定语言的全部模板
    fn load_templates(
        &self,
        locale: &str,
    ) -> impl Future<Output = XueliResult<PromptTemplateMap>> + Send;

    /// 获取指定的单个模板
    fn get_template(
        &self,
        locale: &str,
        name: &str,
    ) -> impl Future<Output = XueliResult<String>> + Send;

    /// 渲染模板（支持变量替换）
    fn render(&self, template: &str, variables: &HashMap<&str, &str>) -> String {
        let mut result = template.to_string();
        for (key, value) in variables {
            result = result.replace(&format!("{{{}}}", key), value);
        }
        result
    }
}
