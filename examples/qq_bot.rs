//! xueli-core QQ Bot 示例
//!
//! 演示如何使用 xueli-core 库构建一个 QQ Bot

use std::sync::Arc;

use xueli_core::core::config::XueliConfig;
use xueli_core::core::runtime::BotRuntime;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化 tracing
    tracing_subscriber::fmt()
        .with_env_filter("xueli_core=debug")
        .init();

    // 加载配置
    let config = XueliConfig::default();

    // 创建运行时
    let runtime = BotRuntime::new(config);
    runtime
        .init()
        .await
        .map_err(|e| format!("初始化失败: {}", e))?;

    tracing::info!("xueli-core QQ Bot 示例已启动");

    // 保持运行
    tokio::signal::ctrl_c().await?;
    runtime.shutdown().await?;

    Ok(())
}
