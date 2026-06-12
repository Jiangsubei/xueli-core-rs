use std::collections::HashMap;
use std::sync::RwLock;

use crate::core::errors::XueliResult;
use crate::core::platform_types::{EventType, InboundEvent};

/// 事件上下文 — 携带事件数据与调度元信息
#[derive(Debug, Clone)]
pub struct EventContext {
    pub event: InboundEvent,
    pub should_handle: bool,
    pub skip_reason: Option<String>,
}

impl EventContext {
    pub fn new(event: InboundEvent) -> Self {
        Self {
            event,
            should_handle: true,
            skip_reason: None,
        }
    }
}

/// 事件分发器统计
#[derive(Debug, Clone, Default)]
pub struct EventDispatcherStats {
    pub total_events: u64,
    pub handled_events: u64,
    pub skipped_events: u64,
    pub message_events: u64,
    pub private_message_events: u64,
    pub group_message_events: u64,
    pub notice_events: u64,
    pub request_events: u64,
    pub meta_events: u64,
}

// ── 类型别名 ──────────────────────────────────────────────────────

type PreprocessorFn = Box<dyn Fn(&mut EventContext) + Send + Sync>;
type PostprocessorFn = Box<dyn Fn(&EventContext) + Send + Sync>;
type MessageHandlerFn = Box<dyn Fn(&InboundEvent) -> XueliResult<()> + Send + Sync>;
type AsyncMessageHandlerFn = Box<
    dyn Fn(
            InboundEvent,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = XueliResult<()>> + Send>>
        + Send
        + Sync,
>;

// ── EventDispatcher ────────────────────────────────────────────────

/// 事件分发器 — 将入站事件路由到对应处理器
pub struct EventDispatcher {
    message_handlers: RwLock<Vec<MessageHandlerFn>>,
    group_message_handlers: RwLock<Vec<MessageHandlerFn>>,
    private_message_handlers: RwLock<Vec<MessageHandlerFn>>,
    event_handlers: RwLock<HashMap<String, Vec<MessageHandlerFn>>>,
    preprocessors: RwLock<Vec<PreprocessorFn>>,
    postprocessors: RwLock<Vec<PostprocessorFn>>,
    /// 异步消息处理器
    async_message_handlers: RwLock<Vec<AsyncMessageHandlerFn>>,
    /// 异步系统事件处理器
    async_event_handlers: RwLock<HashMap<String, Vec<AsyncMessageHandlerFn>>>,
    stats: RwLock<EventDispatcherStats>,
}

impl EventDispatcher {
    pub fn new() -> Self {
        Self {
            message_handlers: RwLock::new(Vec::new()),
            group_message_handlers: RwLock::new(Vec::new()),
            private_message_handlers: RwLock::new(Vec::new()),
            event_handlers: RwLock::new(HashMap::new()),
            preprocessors: RwLock::new(Vec::new()),
            postprocessors: RwLock::new(Vec::new()),
            async_message_handlers: RwLock::new(Vec::new()),
            async_event_handlers: RwLock::new(HashMap::new()),
            stats: RwLock::new(EventDispatcherStats::default()),
        }
    }

    // ── 预/后处理器注册 ─────────────────────────────────────────

    pub fn register_preprocessor<F: Fn(&mut EventContext) + Send + Sync + 'static>(&self, f: F) {
        self.preprocessors.write().unwrap().push(Box::new(f));
    }

    pub fn register_postprocessor<F: Fn(&EventContext) + Send + Sync + 'static>(&self, f: F) {
        self.postprocessors.write().unwrap().push(Box::new(f));
    }

    // ── 消息处理器注册 ─────────────────────────────────────────

    pub fn on_message<F: Fn(&InboundEvent) -> XueliResult<()> + Send + Sync + 'static>(
        &self,
        f: F,
    ) {
        self.message_handlers.write().unwrap().push(Box::new(f));
    }

    pub fn on_group_message<F: Fn(&InboundEvent) -> XueliResult<()> + Send + Sync + 'static>(
        &self,
        f: F,
    ) {
        self.group_message_handlers.write().unwrap().push(Box::new(
            move |event: &InboundEvent| -> XueliResult<()> {
                let scope = event
                    .message
                    .as_ref()
                    .map(|m| &m.scope)
                    .or_else(|| event.session.as_ref().map(|s| &s.scope));
                if scope.is_some_and(|s| s.is_group()) {
                    f(event)
                } else {
                    Ok(())
                }
            },
        ));
    }

    pub fn on_private_message<F: Fn(&InboundEvent) -> XueliResult<()> + Send + Sync + 'static>(
        &self,
        f: F,
    ) {
        self.private_message_handlers
            .write()
            .unwrap()
            .push(Box::new(move |event: &InboundEvent| -> XueliResult<()> {
                let scope = event
                    .message
                    .as_ref()
                    .map(|m| &m.scope)
                    .or_else(|| event.session.as_ref().map(|s| &s.scope));
                if scope.is_none_or(|s| s.is_private()) {
                    f(event)
                } else {
                    Ok(())
                }
            }));
    }

    pub fn on_event<F: Fn(&InboundEvent) -> XueliResult<()> + Send + Sync + 'static>(
        &self,
        event_type: &str,
        f: F,
    ) {
        let key = event_type.trim().to_string();
        let mut handlers = self.event_handlers.write().unwrap();
        handlers.entry(key).or_default().push(Box::new(f));
    }

    pub fn on_notice<F: Fn(&InboundEvent) -> XueliResult<()> + Send + Sync + 'static>(&self, f: F) {
        self.on_event("notice", f);
    }

    pub fn on_request<F: Fn(&InboundEvent) -> XueliResult<()> + Send + Sync + 'static>(
        &self,
        f: F,
    ) {
        self.on_event("request", f);
    }

    pub fn on_meta_event<F: Fn(&InboundEvent) -> XueliResult<()> + Send + Sync + 'static>(
        &self,
        f: F,
    ) {
        self.on_event("meta_event", f);
    }

    // ── dispatch 入口 ──────────────────────────────────────────

    /// 分发入站事件 — 根据 event_type 路由到对应处理器
    pub fn dispatch(&self, event: InboundEvent) -> XueliResult<()> {
        let event_type = event.event_type.clone();
        let is_group = event
            .message
            .as_ref()
            .map(|m| m.scope.is_group())
            .or_else(|| event.session.as_ref().map(|s| s.scope.is_group()))
            .unwrap_or(false);

        // 更新统计
        {
            let mut s = self.stats.write().unwrap();
            s.total_events += 1;
        }

        // 构建事件上下文并运行预处理器
        let mut ctx = EventContext::new(event);
        {
            let preprocessors = self.preprocessors.read().unwrap();
            for pre in preprocessors.iter() {
                pre(&mut ctx);
                if !ctx.should_handle {
                    break;
                }
            }
        }

        if !ctx.should_handle {
            self.stats.write().unwrap().skipped_events += 1;
            return Ok(());
        }

        // 根据事件类型路由
        match event_type {
            EventType::Message | EventType::Mention => {
                // 更新消息统计
                {
                    let mut s = self.stats.write().unwrap();
                    s.message_events += 1;
                    if is_group {
                        s.group_message_events += 1;
                    } else {
                        s.private_message_events += 1;
                    }
                }

                // 运行通用消息处理器
                {
                    let handlers = self.message_handlers.read().unwrap();
                    for handler in handlers.iter() {
                        let _ = handler(&ctx.event);
                    }
                }

                // 根据作用域运行群聊/私聊处理器
                if is_group {
                    let handlers = self.group_message_handlers.read().unwrap();
                    for handler in handlers.iter() {
                        let _ = handler(&ctx.event);
                    }
                } else {
                    let handlers = self.private_message_handlers.read().unwrap();
                    for handler in handlers.iter() {
                        let _ = handler(&ctx.event);
                    }
                }

                // 运行后处理器
                self.run_postprocessors(&ctx);

                self.stats.write().unwrap().handled_events += 1;
            }

            ref other => {
                let event_type_str = event_type_str(other);

                // 更新特定统计
                {
                    let mut s = self.stats.write().unwrap();
                    match event_type_str.as_str() {
                        "notice" => s.notice_events += 1,
                        "request" => s.request_events += 1,
                        "meta_event" => s.meta_events += 1,
                        _ => {}
                    }
                }

                // 运行系统事件处理器
                {
                    let eh = self.event_handlers.read().unwrap();
                    if let Some(handlers) = eh.get(&event_type_str) {
                        for handler in handlers {
                            let _ = handler(&ctx.event);
                        }
                    }
                }

                // 运行后处理器
                self.run_postprocessors(&ctx);

                self.stats.write().unwrap().handled_events += 1;
            }
        }

        Ok(())
    }

    // ── 内部方法 ───────────────────────────────────────────────

    fn run_postprocessors(&self, ctx: &EventContext) {
        let postprocessors = self.postprocessors.read().unwrap();
        for post in postprocessors.iter() {
            post(ctx);
        }
    }

    // ── 统计查询 ──────────────────────────────────────────────

    pub fn get_stats(&self) -> EventDispatcherStats {
        self.stats.read().unwrap().clone()
    }

    // ── 异步处理器注册 ─────────────────────────────────────────

    /// 注册异步消息处理器
    pub fn on_async_message<F, Fut>(&self, f: F)
    where
        F: Fn(InboundEvent) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = XueliResult<()>> + Send + 'static,
    {
        self.async_message_handlers
            .write()
            .unwrap()
            .push(Box::new(move |event| Box::pin(f(event))));
    }

    /// 注册异步系统事件处理器
    pub fn on_async_event<F, Fut>(&self, event_type: &str, f: F)
    where
        F: Fn(InboundEvent) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = XueliResult<()>> + Send + 'static,
    {
        let key = event_type.trim().to_string();
        let mut handlers = self.async_event_handlers.write().unwrap();
        handlers
            .entry(key)
            .or_default()
            .push(Box::new(move |event| Box::pin(f(event))));
    }

    // ── 异步分发入口 ──────────────────────────────────────────

    /// 异步分下入站事件 — 预处理 → 处理器 → 后处理管道
    pub async fn dispatch_inbound_event(&self, event: InboundEvent) -> XueliResult<()> {
        {
            let mut s = self.stats.write().unwrap();
            s.total_events += 1;
        }

        let mut ctx = EventContext::new(event.clone());

        // 运行预处理器
        {
            let preprocessors = self.preprocessors.read().unwrap();
            for pre in preprocessors.iter() {
                pre(&mut ctx);
                if !ctx.should_handle {
                    break;
                }
            }
        }

        if !ctx.should_handle {
            self.stats.write().unwrap().skipped_events += 1;
            return Ok(());
        }

        let event_type = ctx.event.event_type.clone();
        let is_group = ctx
            .event
            .message
            .as_ref()
            .map(|m| m.scope.is_group())
            .or_else(|| ctx.event.session.as_ref().map(|s| s.scope.is_group()))
            .unwrap_or(false);

        // 更新消息统计
        match event_type {
            EventType::Message | EventType::Mention => {
                let mut s = self.stats.write().unwrap();
                s.message_events += 1;
                if is_group {
                    s.group_message_events += 1;
                } else {
                    s.private_message_events += 1;
                }
            }
            ref other => {
                let event_type_str = event_type_str(other);
                let mut s = self.stats.write().unwrap();
                match event_type_str.as_str() {
                    "notice" => s.notice_events += 1,
                    "request" => s.request_events += 1,
                    "meta_event" => s.meta_events += 1,
                    _ => {}
                }
            }
        }

        // 运行异步消息处理器
        {
            let handlers = self.async_message_handlers.read().unwrap();
            for handler in handlers.iter() {
                if let Err(e) = handler(ctx.event.clone()).await {
                    tracing::error!("[调度器] 异步处理器执行失败: {}", e);
                }
            }
        }

        // 运行同步消息处理器
        {
            let handlers = self.message_handlers.read().unwrap();
            for handler in handlers.iter() {
                let _ = handler(&ctx.event);
            }
        }

        // 运行后处理器
        self.run_postprocessors(&ctx);

        self.stats.write().unwrap().handled_events += 1;
        Ok(())
    }

    /// 异步分发系统事件
    pub async fn dispatch_system_event(&self, event: InboundEvent) -> XueliResult<()> {
        {
            let mut s = self.stats.write().unwrap();
            s.total_events += 1;
        }

        let mut ctx = EventContext::new(event.clone());

        // 运行预处理器
        {
            let preprocessors = self.preprocessors.read().unwrap();
            for pre in preprocessors.iter() {
                pre(&mut ctx);
                if !ctx.should_handle {
                    break;
                }
            }
        }

        if !ctx.should_handle {
            self.stats.write().unwrap().skipped_events += 1;
            return Ok(());
        }

        let event_type_str = event_type_str(&event.event_type);

        // 更新统计
        {
            let mut s = self.stats.write().unwrap();
            match event_type_str.as_str() {
                "notice" => s.notice_events += 1,
                "request" => s.request_events += 1,
                "meta_event" => s.meta_events += 1,
                _ => {}
            }
        }

        // 运行异步系统事件处理器
        {
            let handlers = self.async_event_handlers.read().unwrap();
            if let Some(async_handlers) = handlers.get(&event_type_str) {
                for handler in async_handlers.iter() {
                    if let Err(e) = handler(ctx.event.clone()).await {
                        tracing::error!("[调度器] 异步系统事件处理器执行失败: {}", e);
                    }
                }
            }
        }

        // 运行同步系统事件处理器
        {
            let eh = self.event_handlers.read().unwrap();
            if let Some(handlers) = eh.get(&event_type_str) {
                for handler in handlers {
                    let _ = handler(&ctx.event);
                }
            }
        }

        // 运行后处理器
        self.run_postprocessors(&ctx);

        self.stats.write().unwrap().handled_events += 1;
        Ok(())
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// 将 EventType 转换为系统事件字符串
fn event_type_str(et: &EventType) -> String {
    match et {
        EventType::Message => "message".to_string(),
        EventType::Mention => "message".to_string(),
        EventType::JoinGroup => "notice".to_string(),
        EventType::LeaveGroup => "notice".to_string(),
        EventType::Heartbeat => "meta_event".to_string(),
        EventType::Other(s) => s.clone(),
    }
}
