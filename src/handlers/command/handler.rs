// 命令处理器 — 注册表驱动的命令调度器
// 对应 Python 版 xueli/src/handlers/command/handler.py

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::core::config::XueliConfig;
use crate::core::metrics::RuntimeMetrics;
use crate::handlers::command::registry::{CommandContext, CommandRegistry, CommandSpec};

/// 状态提供者：返回当前运行状态键值对
pub type StatusProvider = Arc<dyn Fn() -> HashMap<String, serde_json::Value> + Send + Sync>;

/// 重置回调：清空指定事件的会话上下文
pub type ResetCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// 命令处理器
///
/// 内置命令：
/// - `/help` (/帮助, 帮助) — 查看帮助
/// - `/status` (/状态) — 查看运行状态
/// - `/reset` (/清除, /清空) — 清空会话上下文
pub struct CommandHandler {
    registry: Mutex<CommandRegistry>,
    status_provider: Option<StatusProvider>,
    metrics: Option<Arc<Mutex<RuntimeMetrics>>>,
    config: Arc<XueliConfig>,
    reset_callback: Option<ResetCallback>,
    /// 会话计数回调（可选，由下游注入）
    active_session_counter: Option<Arc<dyn Fn() -> usize + Send + Sync>>,
}

impl CommandHandler {
    pub fn new(
        config: Arc<XueliConfig>,
        metrics: Option<Arc<Mutex<RuntimeMetrics>>>,
        reset_callback: Option<ResetCallback>,
    ) -> Self {
        let mut registry = CommandRegistry::new();
        // 注册内置命令时先用占位 execute，后续在 set_self_references 中替换
        let help_spec = make_builtin_help_spec();
        let status_spec = make_builtin_status_spec();
        let reset_spec = make_builtin_reset_spec();
        registry.register(help_spec, true);
        registry.register(status_spec, true);
        registry.register(reset_spec, true);

        Self {
            registry: Mutex::new(registry),
            status_provider: None,
            metrics,
            config,
            reset_callback,
            active_session_counter: None,
        }
    }

    /// 重新注册内置命令（用于需要闭包捕获 self 的场景）
    ///
    /// 在构造后调用一次，将 help/status/reset 的执行体替换为正确的闭包。
    pub fn init_builtins(&self) {
        // 先在不持有锁的情况下创建命令规格
        let help_spec = self.make_help_spec();
        let status_spec = self.make_status_spec();
        let reset_spec = self.make_reset_spec();

        let mut reg = self.registry.lock().unwrap();
        reg.unregister(&make_builtin_help_spec());
        reg.unregister(&make_builtin_status_spec());
        reg.unregister(&make_builtin_reset_spec());

        reg.register(help_spec, true);
        reg.register(status_spec, true);
        reg.register(reset_spec, true);
    }

    pub fn set_status_provider(&mut self, provider: StatusProvider) {
        self.status_provider = Some(provider);
    }

    pub fn set_active_session_counter(&mut self, counter: Arc<dyn Fn() -> usize + Send + Sync>) {
        self.active_session_counter = Some(counter);
    }

    /// 处理命令文本，返回响应文本或 None（表示不匹配任何命令）
    pub fn handle(&self, text: &str) -> Option<String> {
        let reg = self.registry.lock().unwrap();
        let spec = reg.r#match(text)?;

        // 记录命令命中
        if let Some(ref metrics) = self.metrics {
            if let Ok(m) = metrics.lock() {
                m.record_reply_sent(0.0);
            }
        }

        let ctx = CommandContext {
            raw_text: text.to_string(),
        };
        Some((spec.execute)(&ctx))
    }

    /// 构建帮助文本
    pub fn get_help_text(&self) -> String {
        let assistant_name = self.assistant_name();
        let intro_lines = vec![
            "私聊直接发消息即可。".to_string(),
            "群聊请 @ 机器人，或在配置中开启更主动的群聊回复。".to_string(),
            String::new(),
            format!("当前助手：{}", assistant_name),
        ];
        let reg = self.registry.lock().unwrap();
        reg.build_help_text(&format!("{} 使用帮助", assistant_name), &intro_lines)
    }

    /// 构建状态文本
    pub fn get_status_text(&self) -> String {
        let status = self.snapshot_status();
        let assistant_name = self.assistant_name();
        let active_sessions = self
            .active_session_counter
            .as_ref()
            .map(|c| c())
            .unwrap_or(0);

        let mut lines = vec![
            format!("{} 状态", assistant_name),
            format!(
                "运行状态：{}",
                if status
                    .get("ready")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    "就绪"
                } else {
                    "未就绪"
                }
            ),
            format!(
                "连接状态：{}",
                if status
                    .get("connected")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    "已连接"
                } else {
                    "未连接"
                }
            ),
        ];

        if let Some(m) = status.get("uptime_seconds").and_then(|v| v.as_u64()) {
            lines.push(format!("运行时长：{} 秒", m));
        }
        if let Some(m) = status.get("active_message_tasks").and_then(|v| v.as_u64()) {
            lines.push(format!("活跃消息任务：{}", m));
        }
        lines.push(format!("活跃会话数：{}", active_sessions));

        if let Some(ref metrics) = self.metrics {
            if let Ok(m) = metrics.lock() {
                let snap = m.snapshot();
                let msg_recv = snap
                    .get("total_messages_received")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let msg_sent = snap
                    .get("total_replies_sent")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let ai_calls = snap
                    .get("total_ai_calls")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let tokens = snap
                    .get("total_tokens_used")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let avg_latency = snap
                    .get("avg_response_latency_ms")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let errors = snap
                    .get("error_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let ignored = snap
                    .get("total_ignored")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                lines.push(format!("消息接收/回复：{} / {}", msg_recv, msg_sent));
                lines.push(format!("AI 调用次数/token：{} / {}", ai_calls, tokens));
                lines.push(format!("平均延迟：{:.0} ms", avg_latency));
                lines.push(format!("错误计数：{}", errors));
                lines.push(format!("忽略消息数：{}", ignored));
            }
        }

        lines.push(format!(
            "消息长度限制：{} 字符",
            self.config.reply.max_reply_chars
        ));

        lines.join("\n")
    }

    /// 执行重置命令（供外部直接调用）
    pub fn execute_reset(&self) -> String {
        if let Some(ref cb) = self.reset_callback {
            cb("reset");
        }
        "对话历史已清空。".to_string()
    }

    // ============ 私有方法 ============

    fn assistant_name(&self) -> String {
        let name = self.config.identity.name.trim();
        if name.is_empty() {
            "助手".to_string()
        } else {
            name.to_string()
        }
    }

    fn snapshot_status(&self) -> HashMap<String, serde_json::Value> {
        if let Some(ref provider) = self.status_provider {
            return provider();
        }
        let mut status = HashMap::new();
        status.insert("ready".into(), serde_json::json!(false));
        status.insert("connected".into(), serde_json::json!(false));
        status.insert("uptime_seconds".into(), serde_json::json!(0));
        status.insert("active_message_tasks".into(), serde_json::json!(0));
        status
    }

    fn make_help_spec(&self) -> CommandSpec {
        let help_text = self.get_help_text();
        CommandSpec::new(
            "/help",
            vec!["/帮助".into(), "帮助".into()],
            "查看帮助信息",
            Box::new(move |_ctx| help_text.clone()),
        )
    }

    fn make_status_spec(&self) -> CommandSpec {
        let status_text = self.get_status_text();
        CommandSpec::new(
            "/status",
            vec!["/状态".into()],
            "查看运行状态与指标",
            Box::new(move |_ctx| status_text.clone()),
        )
    }

    fn make_reset_spec(&self) -> CommandSpec {
        let reset_callback = self.reset_callback.clone();
        CommandSpec::new(
            "/reset",
            vec!["/清除".into(), "/清空".into()],
            "清空当前会话上下文",
            Box::new(move |_ctx| {
                if let Some(ref cb) = reset_callback {
                    cb("reset");
                }
                "对话历史已清空。".to_string()
            }),
        )
    }
}

// ============ 占位命令规格（init_builtins 调用前使用） ============

fn make_builtin_help_spec() -> CommandSpec {
    CommandSpec::new(
        "/help",
        vec!["/帮助".into(), "帮助".into()],
        "查看帮助信息",
        Box::new(|_| "".to_string()),
    )
}

fn make_builtin_status_spec() -> CommandSpec {
    CommandSpec::new(
        "/status",
        vec!["/状态".into()],
        "查看运行状态与指标",
        Box::new(|_| "".to_string()),
    )
}

fn make_builtin_reset_spec() -> CommandSpec {
    CommandSpec::new(
        "/reset",
        vec!["/清除".into(), "/清空".into()],
        "清空当前会话上下文",
        Box::new(|_| "".to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Arc<XueliConfig> {
        let mut config = XueliConfig::default();
        config.identity.name = "测试助手".into();
        Arc::new(config)
    }

    #[test]
    fn test_handle_help_command() {
        let config = test_config();
        let handler = CommandHandler::new(config, None, None);
        handler.init_builtins();

        let result = handler.handle("/help").expect("应匹配 /help");
        assert!(result.contains("使用帮助"));
        assert!(result.contains("测试助手"));
    }

    #[test]
    fn test_handle_help_alias() {
        let config = test_config();
        let handler = CommandHandler::new(config, None, None);
        handler.init_builtins();

        let result = handler.handle("帮助").expect("应匹配别名");
        assert!(result.contains("使用帮助"));
    }

    #[test]
    fn test_handle_status_command() {
        let config = test_config();
        let handler = CommandHandler::new(config, None, None);
        handler.init_builtins();

        let result = handler.handle("/status").expect("应匹配 /status");
        assert!(result.contains("状态"));
        assert!(result.contains("未就绪"));
    }

    #[test]
    fn test_handle_reset_command() {
        let config = test_config();
        let handler = CommandHandler::new(config, None, None);
        handler.init_builtins();

        let result = handler.handle("/reset").expect("应匹配 /reset");
        assert_eq!(result, "对话历史已清空。");
    }

    #[test]
    fn test_handle_unknown_command() {
        let config = test_config();
        let handler = CommandHandler::new(config, None, None);
        handler.init_builtins();

        assert!(handler.handle("/unknown").is_none());
    }

    #[test]
    fn test_handle_reset_alias_clear() {
        let config = test_config();
        let handler = CommandHandler::new(config, None, None);
        handler.init_builtins();

        let result = handler.handle("/清除").expect("应匹配 /清除");
        assert_eq!(result, "对话历史已清空。");
    }

    #[test]
    fn test_assistant_name_fallback() {
        let mut config = XueliConfig::default();
        config.identity.name = String::new();
        let handler = CommandHandler::new(Arc::new(config), None, None);
        handler.init_builtins();

        let result = handler.handle("/help").unwrap();
        assert!(result.contains("助手"));
    }
}
