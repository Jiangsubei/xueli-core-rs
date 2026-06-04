use std::sync::Arc;

use crate::prelude::XueliResult;
use crate::core::scope::ChatScope;
use crate::traits::prompt_template::PromptTemplateLoader;

/// ReplyAgent 提示词构建器
///
/// 对应 Python 版 `xueli/src/handlers/reply/prompt_builder.py`
pub struct ReplyPromptBuilder<L: PromptTemplateLoader> {
    template_loader: Arc<L>,
    locale: String,
}

impl<L: PromptTemplateLoader> ReplyPromptBuilder<L> {
    pub fn new(template_loader: Arc<L>, locale: impl Into<String>) -> Self {
        Self {
            template_loader,
            locale: locale.into(),
        }
    }

    /// 构建系统提示词
    pub async fn build_system_prompt(
        &self,
        identity: &str,
        scope: &ChatScope,
        style_guidance: &str,
        person_facts: &[String],
        memories: &[String],
    ) -> String {
        let stable = self
            .build_stable_system_prompt(identity, &scope)
            .await
            .unwrap_or_else(|_| self.fallback_system_prompt(identity, &scope));
        let dynamic = self.build_dynamic_context(style_guidance, person_facts, memories);

        if dynamic.is_empty() {
            stable
        } else {
            format!("{}\n\n{}", stable, dynamic)
        }
    }

    /// 构建稳定部分（模板文件）
    async fn build_stable_system_prompt(
        &self,
        identity: &str,
        scope: &ChatScope,
    ) -> XueliResult<String> {
        let session_label = if scope.is_group() { "群聊" } else { "私聊" };

        let scene_file = if scope.is_group() {
            "scene_guidance_group.prompt"
        } else {
            "scene_guidance_private.prompt"
        };

        let scene_guidance = self
            .template_loader
            .get_template(&self.locale, scene_file)
            .await
            .unwrap_or_default();

        let base = self
            .template_loader
            .get_template(&self.locale, "reply_agent_system_base.prompt")
            .await
            .unwrap_or_else(|_| String::new());

        let rendered = if base.is_empty() {
            self.fallback_system_prompt(identity, scope)
        } else {
            let mut vars = std::collections::HashMap::new();
            vars.insert("identity", identity);
            vars.insert("session_label", session_label);
            vars.insert("scene_guidance", &scene_guidance);
            self.template_loader.render(&base, &vars)
        };

        Ok(rendered)
    }

    /// 构建动态上下文
    fn build_dynamic_context(
        &self,
        style_guidance: &str,
        person_facts: &[String],
        memories: &[String],
    ) -> String {
        let mut parts: Vec<String> = Vec::new();

        if !style_guidance.is_empty() {
            parts.push(format!("【回复风格指引】\n{}", style_guidance));
        }

        if !person_facts.is_empty() {
            parts.push(format!("【用户已知信息】\n{}", person_facts.join("\n")));
        }

        if !memories.is_empty() {
            parts.push(format!("【长期记忆】\n{}", memories.join("\n")));
        }

        parts.join("\n\n")
    }

    /// 兜底提示词（模板加载失败时使用）
    fn fallback_system_prompt(&self, identity: &str, scope: &ChatScope) -> String {
        let session_label = if scope.is_group() { "群聊" } else { "私聊" };
        let scene = if scope.is_group() {
            "群聊场景：保持轻量参与，只在被点名或话题自然轮到你时回应。回复要短（一句到两句），不要长篇大论。"
        } else {
            "私聊场景：自然亲切地回复，但要围绕当前消息，不要假装一直在连续对话。"
        };
        format!(
            "{}\n\n当前会话类型：{session_label}。\n\n\
            你拥有以下工具：\n\
            - reply(text, segments?) — 发送回复文本。调用后本轮结束\n\
            - query_memory(query) — 查询关于当前用户的相关记忆\n\
            - query_person(name) — 查询用户档案\n\
            - view_message(msg_id) — 查看指定消息的完整内容\n\
            - tool_search(query) — 搜索可用工具\n\
            调用 reply 后本轮结束。\n\n\
            {scene}",
            identity
        )
    }
}
