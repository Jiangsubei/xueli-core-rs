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
use crate::core::metrics::RuntimeMetrics;
use crate::handlers::message_handler::MessageHandler;
use crate::handlers::prompt_builder::ReplyPromptBuilder;
use crate::memory::manager::MemoryManager;
use crate::memory::stores::memory_item::SqliteMemoryItemStore;
use crate::memory::stores::person_fact::SqlitePersonFactStore;
use crate::prelude::XueliResult;
use crate::services::ai_client::DefaultAIClient;
use crate::services::prompt_loader::NoopPromptTemplateLoader;
use crate::traits::platform_adapter::PlatformAdapter;

/// 组装完成的运行时组件
pub struct BotRuntimeComponents<P: PlatformAdapter> {
    /// 消息处理器
    pub message_handler: Arc<MessageHandler<DefaultAIClient, P, NoopPromptTemplateLoader>>,
    /// 记忆管理器
    pub memory_manager: Option<Arc<MemoryManager>>,
    /// 人格事实存储
    pub person_fact_store: Option<Arc<SqlitePersonFactStore>>,
    /// 指标收集器
    pub metrics: Arc<RuntimeMetrics>,
    /// 生命周期管理器
    pub lifecycle: LifecycleManager,
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

        // 初始化 AI 客户端
        let model_config = Arc::new(self.config.model.clone());
        let ai_client = Arc::new(DefaultAIClient::new(model_config)?);

        // 初始化提示词模板加载器
        let template_loader = Arc::new(NoopPromptTemplateLoader);

        // 初始化回复提示词构建器
        let prompt_builder =
            ReplyPromptBuilder::<NoopPromptTemplateLoader>::new(template_loader.clone(), "zh-CN");

        // 初始化内存存储（使用 MemoryItemStore 实现 MemoryStore trait）
        let mem_store = Arc::new(SqliteMemoryItemStore::new(data_path)?);

        // 初始化记忆管理器
        let memory_config = &self.config.memory;
        let memory_manager: Option<Arc<MemoryManager>> =
            if memory_config.extraction_min_messages > 0 {
                info!("[启动] 记忆模块：已启用");
                let mgr = Arc::new(MemoryManager::new(
                    Arc::new(memory_config.clone()),
                    mem_store.clone(),
                )?);
                info!("[启动] 记忆管理器初始化完成");
                Some(mgr)
            } else {
                info!("[启动] 记忆模块：未启用（extraction_min_messages <= 0）");
                None
            };

        // 初始化人格事实存储
        let person_fact_store: Option<Arc<SqlitePersonFactStore>> = {
            info!("[启动] 人格事实存储：已启用");
            Some(Arc::new(SqlitePersonFactStore::new(data_path)?))
        };

        let pfs_for_handler = Arc::new(SqlitePersonFactStore::new(data_path)?);

        let mgr_for_handler = memory_manager.clone().unwrap_or_else(|| {
            Arc::new(
                MemoryManager::new(
                    Arc::new(memory_config.clone()),
                    Arc::new(SqliteMemoryItemStore::new(data_path).unwrap()),
                )
                .unwrap(),
            )
        });

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
        })
    }

    /// 输出运行时配置摘要
    fn log_runtime_config(&self) {
        info!(
            "[启动] 运行配置：助手={}，回复模型={}，地址={}",
            self.config.identity.name, self.config.model.primary_model, self.config.model.api_base,
        );

        if let Some(ref vision_model) = self.config.model.vision_model {
            info!("[启动] 视觉服务：模型={}", vision_model);
        }

        info!("[启动] 群聊规划：仅@回复={}", true);

        if self.config.memory.extraction_min_messages > 0 {
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
