// 命令注册表
// 对应 Python 版 xueli/src/handlers/command/registry.py

use std::collections::{HashMap, HashSet};

use crate::core::platform_types::InboundEvent;

/// 命令执行上下文
#[derive(Debug, Clone)]
pub struct CommandContext {
    /// 用户原始输入文本
    pub raw_text: String,
    /// 触发命令的入站事件
    pub event: Option<InboundEvent>,
}

/// 命令执行函数类型
pub type CommandExecutor = Box<dyn Fn(&CommandContext) -> String + Send + Sync>;

/// 命令规格
pub struct CommandSpec {
    /// 命令名称（如 "/help"）
    pub name: String,
    /// 别名列表
    pub aliases: Vec<String>,
    /// 命令描述
    pub description: String,
    /// 用法说明（可选）
    pub usage: String,
    /// 执行函数
    pub execute: CommandExecutor,
}

impl CommandSpec {
    pub fn new(
        name: impl Into<String>,
        aliases: Vec<String>,
        description: impl Into<String>,
        execute: CommandExecutor,
    ) -> Self {
        Self {
            name: name.into(),
            aliases,
            description: description.into(),
            usage: String::new(),
            execute,
        }
    }

    /// 所有别名（包含 name 自身）
    pub fn all_aliases(&self) -> Vec<&str> {
        let mut result: Vec<&str> = vec![&self.name];
        result.extend(self.aliases.iter().map(|s| s.as_str()));
        result
    }
}

/// 命令注册表
///
/// 支持通过名称/别名匹配命令，内置命令不可被外部覆盖。
pub struct CommandRegistry {
    commands: HashMap<String, CommandSpec>,
    alias_map: HashMap<String, String>, // 规范化别名 → 命令名称
    builtin_names: HashSet<String>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
            alias_map: HashMap::new(),
            builtin_names: HashSet::new(),
        }
    }

    /// 注册命令
    pub fn register(&mut self, spec: CommandSpec, builtin: bool) {
        if !builtin && self.builtin_names.contains(&spec.name) {
            tracing::warn!("命令 {} 是内置命令，外部不可覆盖", spec.name);
            return;
        }
        let name = spec.name.clone();
        if builtin {
            self.builtin_names.insert(name.clone());
        }
        for alias in spec.all_aliases() {
            let normalized = Self::normalize(alias);
            if !normalized.is_empty() {
                self.alias_map.insert(normalized, name.clone());
            }
        }
        self.commands.insert(name, spec);
    }

    /// 判断命令是否为内置命令
    pub fn is_builtin(&self, name: &str) -> bool {
        self.builtin_names.contains(name)
    }

    /// 取消注册
    pub fn unregister(&mut self, spec: &CommandSpec) {
        self.commands.remove(&spec.name);
        for alias in spec.all_aliases() {
            let normalized = Self::normalize(alias);
            if self
                .alias_map
                .get(&normalized)
                .map(|n| n == &spec.name)
                .unwrap_or(false)
            {
                self.alias_map.remove(&normalized);
            }
        }
    }

    /// 匹配命令
    pub fn r#match(&self, text: &str) -> Option<&CommandSpec> {
        let normalized = Self::normalize(text);
        if normalized.is_empty() {
            return None;
        }
        let command_token = normalized.split_whitespace().next().unwrap_or("");
        self.alias_map
            .get(command_token)
            .and_then(|name| self.commands.get(name))
    }

    /// 列出所有命令
    pub fn list_commands(&self) -> Vec<&CommandSpec> {
        let mut names: Vec<&String> = self.commands.keys().collect();
        names.sort();
        names.iter().filter_map(|n| self.commands.get(*n)).collect()
    }

    /// 构建帮助文本
    pub fn build_help_text(&self, title: &str, intro_lines: &[String]) -> String {
        let mut lines: Vec<String> = Vec::new();
        lines.push(title.to_string());
        lines.push(String::new());
        for line in intro_lines {
            if !line.is_empty() {
                lines.push(line.clone());
            }
        }
        lines.push("可用命令：".to_string());
        for spec in self.list_commands() {
            let alias_text = spec.all_aliases().join(" / ");
            let usage_suffix = if spec.usage.is_empty() {
                String::new()
            } else {
                format!(" 用法：{}", spec.usage)
            };
            lines.push(format!(
                "- {}：{}{}",
                alias_text, spec.description, usage_suffix
            ));
        }
        lines.join("\n")
    }

    /// 规范化命令文本
    pub fn normalize(text: &str) -> String {
        text.trim().to_lowercase()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_spec(name: &str, aliases: Vec<&str>) -> CommandSpec {
        let name_owned = name.to_string();
        CommandSpec::new(
            name,
            aliases.iter().map(|s| s.to_string()).collect(),
            "test",
            Box::new(move |_| format!("executed {}", name_owned)),
        )
    }

    #[test]
    fn test_register_and_match_exact() {
        let mut reg = CommandRegistry::new();
        reg.register(make_spec("/help", vec!["帮助"]), true);
        let spec = reg.r#match("/help").unwrap();
        assert_eq!(spec.name, "/help");
    }

    #[test]
    fn test_match_alias() {
        let mut reg = CommandRegistry::new();
        reg.register(make_spec("/help", vec!["帮助"]), true);
        let spec = reg.r#match("帮助").unwrap();
        assert_eq!(spec.name, "/help");
    }

    #[test]
    fn test_match_case_insensitive() {
        let mut reg = CommandRegistry::new();
        reg.register(make_spec("/help", vec![]), true);
        let spec = reg.r#match("/Help").unwrap();
        assert_eq!(spec.name, "/help");
    }

    #[test]
    fn test_match_none() {
        let mut reg = CommandRegistry::new();
        reg.register(make_spec("/help", vec![]), true);
        assert!(reg.r#match("/unknown").is_none());
    }

    #[test]
    fn test_unregister() {
        let mut reg = CommandRegistry::new();
        let spec = make_spec("/help", vec!["帮助"]);
        reg.register(spec, true);

        let spec_ref = reg.r#match("/help").unwrap();
        let unreg = CommandSpec::new(
            spec_ref.name.clone(),
            spec_ref.aliases.clone(),
            spec_ref.description.clone(),
            Box::new(|_| String::new()),
        );
        reg.unregister(&unreg);
        assert!(reg.r#match("/help").is_none());
    }

    #[test]
    fn test_builtin_override_protection() {
        let mut reg = CommandRegistry::new();
        reg.register(make_spec("/help", vec![]), true);
        reg.register(make_spec("/help", vec![]), false); // 不应覆盖内置
        let spec = reg.r#match("/help").unwrap();
        assert_eq!(spec.description, "test"); // 保持原值
    }

    #[test]
    fn test_build_help_text() {
        let mut reg = CommandRegistry::new();
        reg.register(make_spec("/help", vec!["帮助"]), true);
        reg.register(make_spec("/status", vec![]), true);
        let text = reg.build_help_text("帮助", &["一行介绍".into()]);
        assert!(text.contains("帮助"));
        assert!(text.contains("/help"));
        assert!(text.contains("/status"));
        assert!(text.contains("一行介绍"));
    }

    #[test]
    fn test_all_aliases() {
        let spec = make_spec("/help", vec!["帮助", "h"]);
        let aliases: Vec<&str> = spec.all_aliases();
        assert_eq!(aliases.len(), 3);
        assert!(aliases.contains(&"/help"));
        assert!(aliases.contains(&"帮助"));
        assert!(aliases.contains(&"h"));
    }
}
