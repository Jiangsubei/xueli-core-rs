// 命令处理器 — 注册表驱动的命令调度器
// 对应 Python 版 xueli/src/handlers/command/handler.py

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::core::config::XueliConfig;
use crate::core::metrics::RuntimeMetrics;
use crate::core::platform_types::InboundEvent;
use crate::handlers::command::registry::{CommandContext, CommandRegistry, CommandSpec};
use crate::handlers::session_manager::ConversationSessionManager;

/// 状态提供者：返回当前运行状态键值对
pub type StatusProvider = Arc<dyn Fn() -> HashMap<String, serde_json::Value> + Send + Sync>;

/// 重置回调：清空指定事件的会话上下文
pub type ResetCallback = Arc<dyn Fn(&InboundEvent) + Send + Sync>;

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
    session_manager: Arc<ConversationSessionManager>,
}

impl CommandHandler {
    pub fn new(
        config: Arc<XueliConfig>,
        session_manager: Arc<ConversationSessionManager>,
        metrics: Option<Arc<Mutex<RuntimeMetrics>>>,
        reset_callback: Option<ResetCallback>,
    ) -> Self {
        let mut registry = CommandRegistry::new();
        // 注册内置命令时先用占位 execute，后续在 init_builtins 中替换
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
            session_manager,
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

    /// 处理命令文本，返回响应文本或 None（表示不匹配任何命令）
    pub fn handle(&self, text: &str, event: &InboundEvent) -> Option<String> {
        let (name, is_builtin) = {
            let reg = self.registry.lock().unwrap();
            let spec = reg.r#match(text)?;
            (spec.name.clone(), reg.is_builtin(&spec.name))
        };

        // 记录命令命中
        if let Some(ref metrics) = self.metrics {
            if let Ok(m) = metrics.lock() {
                m.record_command_hit(&name);
            }
        }

        // 内置命令直接分发，避免闭包需要自引用
        if is_builtin {
            return Some(match name.as_str() {
                "/help" => self.get_help_text(),
                "/status" => self.get_status_text(),
                "/reset" => self.execute_reset_for_event(event),
                _ => {
                    // 理论上不会进入此分支，兜底调用注册表执行体
                    let reg = self.registry.lock().unwrap();
                    let spec = reg.r#match(text)?;
                    let ctx = CommandContext {
                        raw_text: text.to_string(),
                        event: Some(event.clone()),
                    };
                    (spec.execute)(&ctx)
                }
            });
        }

        let reg = self.registry.lock().unwrap();
        let spec = reg.r#match(text)?;
        let ctx = CommandContext {
            raw_text: text.to_string(),
            event: Some(event.clone()),
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

        let status_bool = |key: &str| status.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
        let status_u64 = |key: &str| status.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
        let _status_f64 = |key: &str| status.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);

        let vision_status = match self.config.vision_service_status() {
            "enabled" => "已启用",
            "disabled" => "已禁用",
            _ => "未配置",
        };
        let vision_model = self.config.vision.model.as_deref().unwrap_or("-");

        let mut lines = vec![
            format!("{} 状态", assistant_name),
            format!(
                "运行状态：{}",
                if status_bool("ready") {
                    "就绪"
                } else {
                    "未就绪"
                }
            ),
            format!(
                "连接状态：{}",
                if status_bool("connected") {
                    "已连接"
                } else {
                    "未连接"
                }
            ),
            format!("运行时长：{} 秒", status_u64("uptime_seconds")),
            format!("活跃消息任务：{}", status_u64("active_message_tasks")),
            format!("活跃会话数：{}", status_u64("active_conversations")),
            format!(
                "消息接收/回复：{} / {}",
                status_u64("total_messages_received"),
                status_u64("total_replies_sent")
            ),
            format!("回复分片数：{}", status_u64("reply_parts_sent")),
            format!("命令命中：{}", status_u64("command_hits")),
            format!(
                "群规划 reply/wait/ignore：{} / {} / {}",
                status_u64("planner_reply"),
                status_u64("planner_wait"),
                status_u64("planner_ignore")
            ),
            format!(
                "视觉请求/图片处理/失败：{} / {} / {}",
                status_u64("vision_requests"),
                status_u64("vision_images_processed"),
                status_u64("vision_failures")
            ),
            format!("视觉结果复用：{}", status_u64("vision_reused_from_plan")),
            format!(
                "Memory 读取/共享读取：{} / {}",
                status_u64("memory_reads"),
                status_u64("memory_shared_reads")
            ),
            format!(
                "Memory 场景命中/拒绝：{} / {}",
                status_u64("memory_scene_rule_hits"),
                status_u64("memory_access_denied")
            ),
            format!(
                "Memory 写入/迁移/压缩：{} / {} / {}",
                status_u64("memory_writes"),
                status_u64("memory_migrations"),
                status_u64("memory_compactions")
            ),
            format!("后台任务数：{}", status_u64("background_tasks")),
            format!("AI 服务：{}", self.config.model.api_base),
            format!("模型：{}", self.config.model.primary_model),
            format!("视觉状态：{}", vision_status),
            format!("视觉模型：{}", vision_model),
            format!("响应超时：{} 秒", self.config.bot_behavior.response_timeout),
            format!(
                "消息长度限制：{} 字符",
                self.config.bot_behavior.max_message_length
            ),
        ];

        if let Some(last_error) = status.get("last_error_at").and_then(|v| v.as_str()) {
            if !last_error.is_empty() {
                lines.push(format!("最近错误时间：{}", last_error));
            }
        }

        lines.join("\n")
    }

    // ============ 私有方法 ============

    fn assistant_name(&self) -> String {
        self.config.get_assistant_name().to_string()
    }

    fn snapshot_status(&self) -> HashMap<String, serde_json::Value> {
        if let Some(ref provider) = self.status_provider {
            return provider();
        }
        if let Some(ref metrics) = self.metrics {
            if let Ok(m) = metrics.lock() {
                return m.snapshot();
            }
        }
        let mut status = HashMap::new();
        status.insert("ready".into(), serde_json::json!(false));
        status.insert("connected".into(), serde_json::json!(false));
        status.insert("uptime_seconds".into(), serde_json::json!(0));
        status.insert("active_message_tasks".into(), serde_json::json!(0));
        status.insert("active_conversations".into(), serde_json::json!(0));
        status
    }

    fn execute_reset_for_event(&self, event: &InboundEvent) -> String {
        if let Some(ref cb) = self.reset_callback {
            cb(event);
        }
        let sm = self.session_manager.clone();
        let event = event.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                sm.clear_for_event(&event).await;
            });
        }
        "对话历史已清空。".to_string()
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
        let session_manager = self.session_manager.clone();
        CommandSpec::new(
            "/reset",
            vec!["/清除".into(), "/清空".into()],
            "清空当前会话上下文",
            Box::new(move |ctx| {
                if let Some(ref event) = ctx.event {
                    if let Some(ref cb) = reset_callback {
                        cb(event);
                    }
                    let sm = session_manager.clone();
                    let event = event.clone();
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        handle.spawn(async move {
                            sm.clear_for_event(&event).await;
                        });
                    }
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
    use crate::core::platform_types::EventType;
    use crate::core::scope::ChatScope;
    use crate::core::types::UserMessage;

    fn test_config() -> Arc<XueliConfig> {
        let mut config = XueliConfig::default();
        config.identity.name = "测试助手".into();
        Arc::new(config)
    }

    fn test_session_manager() -> Arc<ConversationSessionManager> {
        Arc::new(ConversationSessionManager::new(None))
    }

    fn dummy_event(user_id: &str) -> InboundEvent {
        InboundEvent {
            id: "e1".into(),
            platform: "test".into(),
            event_type: EventType::Message,
            message: Some(UserMessage {
                id: "m1".into(),
                sender_id: user_id.into(),
                sender_name: "T".into(),
                text: "hi".into(),
                timestamp: chrono::Utc::now(),
                scope: ChatScope::Private,
                is_mention: false,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn test_handle_help_command() {
        let config = test_config();
        let session_mgr = test_session_manager();
        let handler = CommandHandler::new(config, session_mgr, None, None);
        handler.init_builtins();

        let event = dummy_event("u1");
        let result = handler.handle("/help", &event).expect("应匹配 /help");
        assert!(result.contains("使用帮助"));
        assert!(result.contains("测试助手"));
    }

    #[test]
    fn test_handle_help_alias() {
        let config = test_config();
        let session_mgr = test_session_manager();
        let handler = CommandHandler::new(config, session_mgr, None, None);
        handler.init_builtins();

        let event = dummy_event("u1");
        let result = handler.handle("帮助", &event).expect("应匹配别名");
        assert!(result.contains("使用帮助"));
    }

    #[test]
    fn test_handle_status_command() {
        let config = test_config();
        let session_mgr = test_session_manager();
        let handler = CommandHandler::new(config, session_mgr, None, None);
        handler.init_builtins();

        let event = dummy_event("u1");
        let result = handler.handle("/status", &event).expect("应匹配 /status");
        assert!(result.contains("状态"));
        assert!(result.contains("未就绪"));
        assert!(result.contains("群规划 reply/wait/ignore"));
        assert!(result.contains("视觉请求/图片处理/失败"));
        assert!(result.contains("Memory 读取/共享读取"));
        assert!(result.contains("后台任务数"));
    }

    #[test]
    fn test_handle_reset_command() {
        let config = test_config();
        let session_mgr = test_session_manager();
        let handler = CommandHandler::new(config, session_mgr, None, None);
        handler.init_builtins();

        let event = dummy_event("u1");
        let result = handler.handle("/reset", &event).expect("应匹配 /reset");
        assert_eq!(result, "对话历史已清空。");
    }

    #[test]
    fn test_handle_unknown_command() {
        let config = test_config();
        let session_mgr = test_session_manager();
        let handler = CommandHandler::new(config, session_mgr, None, None);
        handler.init_builtins();

        let event = dummy_event("u1");
        assert!(handler.handle("/unknown", &event).is_none());
    }

    #[test]
    fn test_handle_reset_alias_clear() {
        let config = test_config();
        let session_mgr = test_session_manager();
        let handler = CommandHandler::new(config, session_mgr, None, None);
        handler.init_builtins();

        let event = dummy_event("u1");
        let result = handler.handle("/清除", &event).expect("应匹配 /清除");
        assert_eq!(result, "对话历史已清空。");
    }

    #[test]
    fn test_assistant_name_fallback() {
        let mut config = XueliConfig::default();
        config.identity.name = String::new();
        let session_mgr = test_session_manager();
        let handler = CommandHandler::new(Arc::new(config), session_mgr, None, None);
        handler.init_builtins();

        let event = dummy_event("u1");
        let result = handler.handle("/help", &event).unwrap();
        assert!(result.contains("助手"));
    }

    #[tokio::test]
    async fn test_reset_clears_session() {
        let config = test_config();
        let session_mgr = test_session_manager();
        let handler = CommandHandler::new(config, session_mgr.clone(), None, None);
        handler.init_builtins();

        let event = dummy_event("u_reset");
        let key = session_mgr.get_key_for_event(&event);
        session_mgr
            .add_message(&key, "user", "hello", None, "", "", false)
            .await;
        assert_eq!(session_mgr.count_active().await, 1);

        let result = handler.handle("/reset", &event).expect("应匹配 /reset");
        assert_eq!(result, "对话历史已清空。");

        // 让后台 clear_for_event 任务执行
        tokio::task::yield_now().await;
        assert_eq!(session_mgr.count_active().await, 0);
    }

    #[test]
    fn test_reset_callback_receives_event() {
        let config = test_config();
        let session_mgr = test_session_manager();
        let (tx, rx) = std::sync::mpsc::channel();
        let cb: ResetCallback = Arc::new(move |event: &InboundEvent| {
            let _ = tx.send(
                event
                    .message
                    .as_ref()
                    .map(|m| m.sender_id.clone())
                    .unwrap_or_default(),
            );
        });
        let handler = CommandHandler::new(config, session_mgr, None, Some(cb));
        handler.init_builtins();

        let event = dummy_event("u_callback");
        let result = handler.handle("/reset", &event).expect("应匹配 /reset");
        assert_eq!(result, "对话历史已清空。");

        let received = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("应收到回调事件");
        assert_eq!(received, "u_callback");
    }
}
