use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};

use chrono::Timelike;

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
    /// 空闲后才考虑发送的最小小时数
    idle_hours: f64,
    /// 冷却小时数
    cooldown_hours: f64,
    /// 冷却秒数（内部使用）
    cooldown_seconds: f64,
    /// 可发送时间窗口起始
    time_range_start: String,
    /// 可发送时间窗口结束
    time_range_end: String,
    /// 轮询间隔秒数
    check_interval_seconds: f64,
}

impl ProactiveShareScheduler {
    pub fn new(config: Arc<ProactiveShareConfig>, store: Arc<ProactiveShareStore>) -> Self {
        let idle_hours = config.idle_hours;
        let cooldown_hours = config.cooldown_hours;
        let max_per_day = config.max_per_day;
        let time_range_start = config.time_range_start.clone();
        let time_range_end = config.time_range_end.clone();
        let check_interval_seconds = config.interval_secs as f64;
        let cooldown_seconds = config.cooldown_hours * 3600.0;
        Self {
            idle_hours,
            cooldown_hours,
            max_per_day,
            time_range_start,
            time_range_end,
            check_interval_seconds,
            config,
            store,
            running: Arc::new(RwLock::new(false)),
            last_interaction_at: Arc::new(RwLock::new(0.0)),
            global_cooldown_active: Arc::new(RwLock::new(false)),
            cooldown_seconds,
        }
    }

    /// 设置每日最大发送量
    pub fn with_max_per_day(mut self, max: usize) -> Self {
        self.max_per_day = max;
        self
    }

    /// 检查当前时间是否在可发送时间窗口内
    fn within_time_range(&self) -> bool {
        let (start_min, end_min) = match self.parse_time_range() {
            Some(v) => v,
            None => return true,
        };
        let now = chrono::Local::now().time();
        let current_minutes = now.hour() as i32 * 60 + now.minute() as i32;
        if end_min < start_min {
            current_minutes >= start_min || current_minutes <= end_min
        } else {
            start_min <= current_minutes && current_minutes <= end_min
        }
    }

    fn parse_time_range(&self) -> Option<(i32, i32)> {
        let parts: Vec<&str> = self.time_range_start.split(':').collect();
        if parts.len() != 2 {
            return None;
        }
        let start_h: i32 = parts[0].parse().ok()?;
        let start_m: i32 = parts[1].parse().ok()?;
        let parts: Vec<&str> = self.time_range_end.split(':').collect();
        if parts.len() != 2 {
            return None;
        }
        let end_h: i32 = parts[0].parse().ok()?;
        let end_m: i32 = parts[1].parse().ok()?;
        Some((start_h * 60 + start_m, end_h * 60 + end_m))
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
        let _cooldown = self.global_cooldown_active.clone();
        let idle_secs = self.idle_hours * 3600.0;
        let _cooldown_secs = self.cooldown_seconds;
        let max_pd = self.max_per_day;
        let cooldown_hours = self.cooldown_hours;
        let time_range_start = self.time_range_start.clone();
        let time_range_end = self.time_range_end.clone();
        let check_interval_secs = self.check_interval_seconds;

        tokio::spawn(async move {
            let check_interval = check_interval_secs.max(60.0) as u64;
            let mut ticker = interval(Duration::from_secs(check_interval));
            ticker.tick().await; // 跳过一次

            loop {
                ticker.tick().await;

                {
                    let r = running.read().await;
                    if !*r {
                        break;
                    }
                }

                // 检查时间窗口
                if !within_time_range_static(&time_range_start, &time_range_end) {
                    continue;
                }

                // 检查是否有正在冷却
                if store.is_global_cooldown_active() {
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
                if let Ok(pending) = store.pending_shares_with_cooldown(1, cooldown_hours, &time_range_start, &time_range_end) {
                    for share in pending {
                        // 标记已发送
                        let _ = store.mark_sent(&share.id);
                        // 设置全局冷却
                        store.set_global_cooldown(cooldown_hours);
                        // 记录互动时间
                        let now_ts = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs_f64();
                        let mut last = last_interaction.write().await;
                        *last = now_ts;
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

fn within_time_range_static(time_range_start: &str, time_range_end: &str) -> bool {
    let start_parts: Vec<&str> = time_range_start.split(':').collect();
    let end_parts: Vec<&str> = time_range_end.split(':').collect();
    let (start_min, end_min) = match (start_parts.len(), end_parts.len()) {
        (2, 2) => {
            let start_h: i32 = start_parts[0].parse().unwrap_or(9);
            let start_m: i32 = start_parts[1].parse().unwrap_or(0);
            let end_h: i32 = end_parts[0].parse().unwrap_or(22);
            let end_m: i32 = end_parts[1].parse().unwrap_or(0);
            (start_h * 60 + start_m, end_h * 60 + end_m)
        }
        _ => return true,
    };
    let now = chrono::Local::now().time();
    let current_minutes = now.hour() as i32 * 60 + now.minute() as i32;
    if end_min < start_min {
        current_minutes >= start_min || current_minutes <= end_min
    } else {
        start_min <= current_minutes && current_minutes <= end_min
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
