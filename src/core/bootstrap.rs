/// 机器人启动辅助工具 — 用于配置验证、依赖构建和组件装配。
///
/// 对应 Python 版 `xueli/src/core/bootstrap.py`
///
/// 注意：适配器（Adapter）由下游实现 `PlatformAdapter` trait，因此 bootstrap 不包含
/// 适配器创建逻辑。下游项目应自行创建适配器并传入。
use std::path::Path;
use std::sync::Arc;

use tracing::{info, warn};

use crate::core::config::XueliConfig;
use crate::core::lifecycle::LifecycleManager;
use crate::core::log_labels::LOG_STARTUP_INFO;
use crate::core::metrics::RuntimeMetrics;
use crate::handlers::message_handler::MessageHandler;
use crate::handlers::prompt_builder::ReplyPromptBuilder;
use crate::memory::manager::MemoryManager;
use crate::memory::stores::memory_item::SqliteMemoryItemStore;
use crate::memory::stores::person_fact::SqlitePersonFactStore;
use crate::prelude::XueliResult;
use crate::services::ai_client::DefaultAIClient;
use crate::services::prompt_loader::NoopPromptTemplateLoader;
use crate::services::token_counter::TokenCounter;
use crate::traits::platform_adapter::PlatformAdapter;

/// 组装完成的运行时组件
pub struct BotRuntimeComponents<P: PlatformAdapter> {
    /// 消息处理器
    pub message_handler: Arc<MessageHandler<DefaultAIClient, P, NoopPromptTemplateLoader>>,
    /// 记忆管理器
    pub memory_manager: Option<Arc<MemoryManager<NoopPromptTemplateLoader>>>,
    /// 人格事实存储
    pub person_fact_store: Option<Arc<SqlitePersonFactStore>>,
    /// 指标收集器
    pub metrics: Arc<RuntimeMetrics>,
    /// 生命周期管理器
    pub lifecycle: LifecycleManager,
    /// Token 计数器
    pub token_counter: Arc<TokenCounter>,
}

/// 启动引导器 — 验证配置并组装运行时组件。
pub struct BotBootstrapper<P: PlatformAdapter> {
    config: Arc<XueliConfig>,
    platform: Arc<P>,
    /// 数据存储目录（SQLite 数据库文件存放于此）
    data_dir: String,
}

impl<P: PlatformAdapter> BotBootstrapper<P> {
    pub fn new(config: Arc<XueliConfig>, platform: Arc<P>) -> Self {
        Self {
            config,
            platform,
            data_dir: "data".to_string(),
        }
    }

    /// 设置数据存储目录
    pub fn with_data_dir(mut self, dir: impl Into<String>) -> Self {
        self.data_dir = dir.into();
        self
    }

    /// 构建所有运行时组件
    pub async fn build(&self) -> XueliResult<BotRuntimeComponents<P>> {
        self.log_runtime_config();

        let data_path = Path::new(&self.data_dir);
        std::fs::create_dir_all(data_path)
            .map_err(|e| format!("无法创建数据目录 {}: {}", self.data_dir, e))?;

        let metrics = Arc::new(RuntimeMetrics::default());
        let lifecycle = LifecycleManager::new();

        // 初始化 TokenCounter
        let token_counter = self.initialize_token_counter();

        // 初始化 AI 客户端
        let model_config = Arc::new(self.config.model.clone());
        let ai_client = Arc::new(DefaultAIClient::new(model_config)?);

        // 初始化提示词模板加载器
        let template_loader = Arc::new(NoopPromptTemplateLoader);

        // 初始化回复提示词构建器
        let prompt_builder =
            ReplyPromptBuilder::<NoopPromptTemplateLoader>::new(template_loader.clone(), "zh-CN");

        // 初始化记忆管理器
        let memory_manager = self.initialize_memory_manager(&metrics)?;

        // 初始化人格事实存储
        let person_fact_store: Option<Arc<SqlitePersonFactStore>> = {
            info!("[启动] 人格事实存储：已启用");
            Some(Arc::new(SqlitePersonFactStore::new(data_path)?))
        };

        let pfs_for_handler = Arc::new(SqlitePersonFactStore::new(data_path)?);

        let mgr_for_handler = match memory_manager.clone() {
            Some(mgr) => mgr,
            None => Arc::new(MemoryManager::new(
                Arc::new(self.config.memory.clone()),
                Arc::new(SqliteMemoryItemStore::new(data_path)?),
                Arc::new(NoopPromptTemplateLoader),
            )?),
        };

        // 初始化消息处理器
        let message_handler = Arc::new(MessageHandler::new(
            self.config.clone(),
            ai_client,
            self.platform.clone(),
            mgr_for_handler,
            pfs_for_handler,
            template_loader,
            prompt_builder,
        ));

        info!("[启动] 消息处理器初始化完成");

        Ok(BotRuntimeComponents {
            message_handler,
            memory_manager,
            person_fact_store,
            metrics,
            lifecycle,
            token_counter: Arc::new(token_counter),
        })
    }

    /// 初始化 TokenCounter
    fn initialize_token_counter(&self) -> TokenCounter {
        let encoding = &self.config.bot_behavior.token_encoding;
        let counter = TokenCounter::new(encoding).unwrap_or_else(|e| {
            warn!(
                "[启动] TokenCounter 初始化失败 (encoding={}): {}，使用 cl100k 回退",
                encoding, e
            );
            TokenCounter::new_cl100k().unwrap_or_else(|e2| {
                warn!("[启动] TokenCounter cl100k 回退也失败: {}", e2);
                panic!("TokenCounter 初始化完全失败，无法继续");
            })
        });
        if counter.available() {
            info!(target: LOG_STARTUP_INFO, "[启动] TokenCounter 初始化完成，编码={}", encoding);
        } else {
            warn!("[启动] TokenCounter 初始化失败，token 感知上下文管理将回退");
        }
        counter
    }

    /// 初始化记忆管理器
    fn initialize_memory_manager(
        &self,
        _metrics: &Arc<RuntimeMetrics>,
    ) -> XueliResult<Option<Arc<MemoryManager<NoopPromptTemplateLoader>>>> {
        let memory_config = &self.config.memory;

        if !memory_config.enabled {
            info!("[启动] 记忆模块：未启用");
            return Ok(None);
        }

        info!("[启动] 记忆模块：已启用");

        let data_path = Path::new(&self.data_dir);
        let mem_store = Arc::new(SqliteMemoryItemStore::new(data_path)?);

        // 记忆提取模型配置日志
        if self.config.is_memory_extraction_configured() {
            info!("[启动] 记忆提取运行策略：优先使用专用提取模型，失败时回退主模型");
        } else if self.config.is_ai_service_configured() {
            info!("[启动] 记忆提取运行策略：未配置专用提取模型，当前直接使用主模型");
        } else {
            warn!("[启动] 记忆提取运行策略：主模型与专用提取模型均不可用");
        }

        info!(
            "[启动] 记忆配置：自动提取={}，每{}轮提取一次",
            memory_config.auto_extract, memory_config.extract_every_n_turns,
        );

        let mgr = Arc::new(MemoryManager::new(
            Arc::new(memory_config.clone()),
            mem_store,
            Arc::new(NoopPromptTemplateLoader),
        )?);

        info!("[启动] 记忆管理器初始化完成");
        Ok(Some(mgr))
    }

    /// 输出运行时配置摘要
    fn log_runtime_config(&self) {
        let vision_status = self.config.vision_service_status();
        let decision_configured = self.config.is_group_reply_decision_configured();
        let rerank_configured = self.config.is_memory_rerank_configured();

        info!(
            target: LOG_STARTUP_INFO,
            "[启动] 运行配置：助手={}，回复模型={}，地址={}",
            self.config.get_assistant_name(),
            self.config.model.primary_model,
            self.config.model.api_base,
        );

        info!(
            "[启动] 视觉服务：状态={}，模型={}",
            vision_status,
            self.config.vision.model.as_deref().unwrap_or("未配置"),
        );

        info!(
            "[启动] 群聊规划：已配置={}，模型={}，仅@回复={}，兴趣回复={}，上下文预算比例={:.1}",
            decision_configured,
            self.config.group_reply_decision.model,
            self.config.group_reply.only_reply_when_at,
            self.config.group_reply.interest_reply_enabled,
            self.config.bot_behavior.context_token_budget_ratio,
        );

        if rerank_configured {
            info!(
                "[启动] 记忆重排序：已配置，模型={}",
                self.config.memory_rerank.model,
            );
        }

        if self.config.memory.enabled {
            info!(
                "[启动] 记忆提取：已配置，每{}条消息提取",
                self.config.memory.extraction_min_messages
            );
        } else {
            warn!("[启动] 记忆提取：未配置");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::platform_types::{InboundEvent, ReplyAction};
    use async_trait::async_trait;

    struct MockPlatform;
    #[async_trait]
    impl PlatformAdapter for MockPlatform {
        async fn send_action(&self, _action: &ReplyAction) -> XueliResult<()> {
            Ok(())
        }

        fn strip_mentions(&self, text: &str) -> String {
            text.to_string()
        }

        fn extract_mentions(&self, _event: &InboundEvent) -> Vec<String> {
            Vec::new()
        }

        fn resolve_mention_placeholders(&self, text: &str, _mentions: &[String]) -> String {
            text.to_string()
        }

        fn platform_name(&self) -> &str {
            "mock"
        }

        fn parse_event(&self, _raw: &str) -> XueliResult<InboundEvent> {
            Err("mock: not implemented".into())
        }
    }

    /// 测试配置为默认配置时，引导器能成功构建
    #[tokio::test]
    async fn test_build_default_config() {
        let config = Arc::new(XueliConfig::default());
        let platform = Arc::new(MockPlatform);
        let bootstrapper =
            BotBootstrapper::new(config, platform).with_data_dir("/tmp/test_bootstrap_data");

        let result = bootstrapper.build().await;
        assert!(result.is_ok());

        // 清理
        let _ = std::fs::remove_dir_all("/tmp/test_bootstrap_data");
    }
}
