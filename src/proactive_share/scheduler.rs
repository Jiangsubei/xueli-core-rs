use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};

use crate::core::config::ProactiveShareConfig;
use crate::proactive_share::store::ProactiveShareStore;

/// 主动分享调度器 — 定时检查待发送分享并触发发送。
pub struct ProactiveShareScheduler {
    #[allow(dead_code)]
    config: Arc<ProactiveShareConfig>,
    store: Arc<ProactiveShareStore>,
    running: Arc<RwLock<bool>>,
    /// 上次互动时间（避免活跃对话时插播分享）
    last_interaction_at: Arc<RwLock<f64>>,
    /// 全局冷却（发送后间隔一段时间不再发送）
    global_cooldown_active: Arc<RwLock<bool>>,
    /// 每自然日最大发送量
    max_per_day: usize,
    /// 空闲后才考虑发送的最小秒数
    idle_seconds: f64,
    /// 冷却秒数
    cooldown_seconds: f64,
}

impl ProactiveShareScheduler {
    pub fn new(config: Arc<ProactiveShareConfig>, store: Arc<ProactiveShareStore>) -> Self {
        Self {
            config,
            store,
            running: Arc::new(RwLock::new(false)),
            last_interaction_at: Arc::new(RwLock::new(0.0)),
            global_cooldown_active: Arc::new(RwLock::new(false)),
            max_per_day: 3,
            idle_seconds: 600.0,
            cooldown_seconds: 1800.0,
        }
    }

    /// 设置每日最大发送量
    pub fn with_max_per_day(mut self, max: usize) -> Self {
        self.max_per_day = max;
        self
    }

    /// 启动定时调度循环
    pub async fn start(&self) {
        {
            let mut running = self.running.write().await;
            if *running {
                return;
            }
            *running = true;
        }

        let store = self.store.clone();
        let running = self.running.clone();
        let last_interaction = self.last_interaction_at.clone();
        let cooldown = self.global_cooldown_active.clone();
        let idle_secs = self.idle_seconds;
        let cooldown_secs = self.cooldown_seconds;
        let max_pd = self.max_per_day;

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(30));
            ticker.tick().await; // 跳过一次

            loop {
                ticker.tick().await;

                {
                    let r = running.read().await;
                    if !*r {
                        break;
                    }
                }

                // 检查是否有正在冷却
                let in_cooldown = *cooldown.read().await;
                if in_cooldown {
                    continue;
                }

                // 检查是否空闲足够久
                let last = *last_interaction.read().await;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64();
                if now - last < idle_secs {
                    continue;
                }

                // 检查今日已发送量
                let today_sent = store.count_sent_today();
                if today_sent >= max_pd {
                    continue;
                }

                // 获取待发送分享
                if let Ok(pending) = store.pending_shares(1) {
                    if let Some(share) = pending.first() {
                        // 标记已发送
                        let _ = store.mark_sent(&share.id);
                        // 设置冷却
                        {
                            let mut c = cooldown.write().await;
                            *c = true;
                        }
                        // 延迟清除冷却
                        let c = cooldown.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs_f64(cooldown_secs)).await;
                            let mut cooldown = c.write().await;
                            *cooldown = false;
                        });
                    }
                }
            }
        });
    }

    /// 停止调度
    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
    }

    /// 记录互动时间（重置空闲计时）
    pub async fn record_interaction(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let mut last = self.last_interaction_at.write().await;
        *last = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> Arc<ProactiveShareStore> {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("shares.json");
        Arc::new(ProactiveShareStore::new(path.to_str().unwrap()))
    }

    #[tokio::test]
    async fn test_scheduler_start_stop() {
        let config = Arc::new(ProactiveShareConfig::default());
        let scheduler = ProactiveShareScheduler::new(config, make_store());
        scheduler.start().await;
        scheduler.stop().await;
    }

    #[tokio::test]
    async fn test_record_interaction() {
        let config = Arc::new(ProactiveShareConfig::default());
        let scheduler = ProactiveShareScheduler::new(config, make_store());
        scheduler.record_interaction().await;
        let last = *scheduler.last_interaction_at.read().await;
        assert!(last > 0.0);
    }
}
