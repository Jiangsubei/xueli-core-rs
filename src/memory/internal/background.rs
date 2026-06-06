use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};

use crate::core::config::MemoryConfig;

/// 后台协调器 — 离线记忆维护（消化、反思、清理）
///
/// 对应 Python 版 `xueli/src/memory/internal/background_coordinator.py`
pub struct BackgroundCoordinator {
    config: Arc<MemoryConfig>,
    /// 是否正在运行
    running: Arc<RwLock<bool>>,
    /// 回调节点：消化周期触发
    on_digest_tick: Arc<RwLock<Option<Box<dyn Fn() + Send + Sync>>>>,
    /// 回调节点：记忆变更通知
    on_memory_changed: Arc<RwLock<Option<Box<dyn Fn() + Send + Sync>>>>,
    /// 回调节点：insight 生成
    on_insight_generated: Arc<RwLock<Option<Box<dyn Fn(String) + Send + Sync>>>>,
}

impl BackgroundCoordinator {
    pub fn new(config: Arc<MemoryConfig>) -> Self {
        Self {
            config,
            running: Arc::new(RwLock::new(false)),
            on_digest_tick: Arc::new(RwLock::new(None)),
            on_memory_changed: Arc::new(RwLock::new(None)),
            on_insight_generated: Arc::new(RwLock::new(None)),
        }
    }

    /// 设置消化周期回调（每次 ticking 时调用）
    pub async fn set_digest_tick_callback<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut cb = self.on_digest_tick.write().await;
        *cb = Some(Box::new(callback));
    }

    /// 设置记忆变更回调
    pub async fn set_memory_changed_callback<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut cb = self.on_memory_changed.write().await;
        *cb = Some(Box::new(callback));
    }

    /// 设置 insight 生成回调
    pub async fn set_insight_generated_callback<F>(&self, callback: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let mut cb = self.on_insight_generated.write().await;
        *cb = Some(Box::new(callback));
    }

    /// 启动后台消化循环
    ///
    /// 每隔 digest_interval_secs 秒触发一次消化 tick，
    /// 负责：对话摘要更新、记忆提取、人物事实同步、insight 生成。
    pub async fn start(&self, digest_interval_secs: u64) {
        {
            let mut running = self.running.write().await;
            if *running {
                tracing::debug!("[后台协调] 已在运行中，跳过重复启动");
                return;
            }
            *running = true;
        }

        let running = self.running.clone();
        let on_digest_tick = self.on_digest_tick.clone();

        let interval_dur = Duration::from_secs(digest_interval_secs.max(1));

        tokio::spawn(async move {
            let mut ticker = interval(interval_dur);
            // 跳过一次立即触发，等第一个间隔
            ticker.tick().await;

            loop {
                ticker.tick().await;

                // 检查是否仍在运行
                {
                    let r = running.read().await;
                    if !*r {
                        tracing::info!("[后台协调] 消化循环已停止");
                        break;
                    }
                }

                // 触发消化 tick
                {
                    let cb = on_digest_tick.read().await;
                    if let Some(ref callback) = *cb {
                        callback();
                    }
                }

                tracing::debug!("[后台协调] 消化 tick 完成");
            }
        });

        tracing::info!(
            "[后台协调] 已启动消化循环，间隔 {} 秒",
            digest_interval_secs
        );
    }

    /// 停止后台消化循环
    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        if *running {
            *running = false;
            tracing::info!("[后台协调] 正在停止消化循环");
        }
    }

    /// 是否正在运行
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }

    /// 通知外部：记忆已变更
    pub async fn notify_memory_changed(&self) {
        let cb = self.on_memory_changed.read().await;
        if let Some(ref callback) = *cb {
            callback();
        }
    }

    /// 通知外部：产生了新的 insight
    pub async fn notify_insight(&self, insight: String) {
        let cb = self.on_insight_generated.read().await;
        if let Some(ref callback) = *cb {
            callback(insight);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_background_coordinator_start_stop() {
        let config = Arc::new(MemoryConfig {
            enabled: true,
            db_path: ":memory:".to_string(),
            storage_backend: "sqlite".to_string(),
            extraction_min_messages: 5,
            bm25_top_k: 10,
            vector_top_k: 10,
            rerank_top_k: 20,
            dynamic_memory_limit: 8,
            dispute: Default::default(),
            auto_extract: true,
            extract_every_n_turns: 3,
            decay: Default::default(),
            retrieval_weights: Default::default(),
        });
        let coordinator = BackgroundCoordinator::new(config);
        assert!(!coordinator.is_running().await);

        // 设置回调
        let counter = Arc::new(RwLock::new(0));
        let c = counter.clone();
        coordinator
            .set_digest_tick_callback(move || {
                let _ = c.try_write().map(|mut v| *v += 1);
            })
            .await;

        // 启动，间隔很短用于测试
        coordinator.start(1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(coordinator.is_running().await);

        // 等待 tick 触发
        tokio::time::sleep(Duration::from_millis(1200)).await;

        coordinator.stop().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!coordinator.is_running().await);

        let count = *counter.read().await;
        assert!(count >= 1, "期望 tick 至少 1 次，实际 {}", count);
    }

    #[tokio::test]
    async fn test_duplicate_start_prevented() {
        let config = Arc::new(MemoryConfig {
            enabled: true,
            db_path: ":memory:".to_string(),
            storage_backend: "sqlite".to_string(),
            extraction_min_messages: 5,
            bm25_top_k: 10,
            vector_top_k: 10,
            rerank_top_k: 20,
            dynamic_memory_limit: 8,
            dispute: Default::default(),
            auto_extract: true,
            extract_every_n_turns: 3,
            decay: Default::default(),
            retrieval_weights: Default::default(),
        });
        let coordinator = BackgroundCoordinator::new(config);
        coordinator.start(60).await;
        coordinator.start(60).await; // 不应 panic
        assert!(coordinator.is_running().await);
        coordinator.stop().await;
    }
}
