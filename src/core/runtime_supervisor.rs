use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use std::time::Instant;
use tracing::info;

use crate::prelude::XueliResult;

/// 运行时生命周期状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStatus {
    Idle,
    Starting,
    Running,
    Stopping,
    Stopped,
    Error,
}

/// 运行时监督器 — 管理生命周期、健康检查、异常恢复
pub struct RuntimeSupervisor {
    status: RwLock<RuntimeStatus>,
    healthy: AtomicBool,
    started_at: RwLock<Option<Instant>>,
    retry_count: RwLock<u32>,
    max_retries: u32,
    health_check_interval_secs: u64,
}

impl RuntimeSupervisor {
    pub fn new(max_retries: u32, health_check_interval_secs: u64) -> Self {
        Self {
            status: RwLock::new(RuntimeStatus::Idle),
            healthy: AtomicBool::new(false),
            started_at: RwLock::new(None),
            retry_count: RwLock::new(0),
            max_retries,
            health_check_interval_secs,
        }
    }

    /// 启动：Idle → Starting → Running
    pub fn start(&self) -> XueliResult<()> {
        let mut status = self
            .status
            .write()
            .map_err(|e| format!("RuntimeSupervisor 锁故障: {}", e))?;
        if *status != RuntimeStatus::Idle && *status != RuntimeStatus::Stopped {
            return Err(format!("不能从状态 {:?} 启动", *status).into());
        }
        *status = RuntimeStatus::Starting;
        self.healthy.store(true, Ordering::SeqCst);
        self.started_at
            .write()
            .map_err(|e| format!("RuntimeSupervisor 锁故障: {}", e))?
            .replace(Instant::now());
        *status = RuntimeStatus::Running;
        info!("[监督器] 运行时已启动");
        Ok(())
    }

    /// 停止：Running → Stopping → Stopped
    pub fn stop(&self) -> XueliResult<()> {
        let mut status = self
            .status
            .write()
            .map_err(|e| format!("RuntimeSupervisor 锁故障: {}", e))?;
        if *status != RuntimeStatus::Running {
            return Err(format!("不能从状态 {:?} 停止", *status).into());
        }
        *status = RuntimeStatus::Stopping;
        self.healthy.store(false, Ordering::SeqCst);
        *status = RuntimeStatus::Stopped;
        info!("[监督器] 运行时已停止");
        Ok(())
    }

    /// 重启：stop → start，成功时重置重试计数，失败时递增
    pub fn restart(&self) -> XueliResult<()> {
        let was_running = self.get_status() == RuntimeStatus::Running;
        if was_running {
            self.stop()?;
        }
        match self.start() {
            Ok(()) => {
                if let Ok(mut rc) = self.retry_count.write() {
                    *rc = 0;
                }
                info!("[监督器] 运行时已重启");
                Ok(())
            }
            Err(e) => {
                if let Ok(mut rc) = self.retry_count.write() {
                    *rc += 1;
                }
                Err(e)
            }
        }
    }

    pub fn get_status(&self) -> RuntimeStatus {
        *self
            .status
            .read()
            .map_err(|e| format!("RuntimeSupervisor 锁故障: {}", e))
            .unwrap()
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::SeqCst)
    }

    pub fn mark_healthy(&self) {
        self.healthy.store(true, Ordering::SeqCst);
    }

    pub fn mark_unhealthy(&self) {
        self.healthy.store(false, Ordering::SeqCst);
    }

    pub fn mark_error(&self) {
        if let Ok(mut status) = self.status.write() {
            *status = RuntimeStatus::Error;
        }
        self.healthy.store(false, Ordering::SeqCst);
    }

    pub fn uptime_seconds(&self) -> f64 {
        if self.get_status() == RuntimeStatus::Running {
            if let Ok(started) = self.started_at.read() {
                if let Some(start) = *started {
                    return start.elapsed().as_secs_f64();
                }
            }
        }
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let s = RuntimeSupervisor::new(3, 60);
        assert_eq!(s.get_status(), RuntimeStatus::Idle);
        assert!(!s.is_healthy());
        assert_eq!(s.uptime_seconds(), 0.0);
    }

    #[test]
    fn test_start_stop_cycle() {
        let s = RuntimeSupervisor::new(3, 60);
        assert!(s.start().is_ok());
        assert_eq!(s.get_status(), RuntimeStatus::Running);
        assert!(s.is_healthy());
        assert!(s.uptime_seconds() >= 0.0);
        assert!(s.stop().is_ok());
        assert_eq!(s.get_status(), RuntimeStatus::Stopped);
        assert!(!s.is_healthy());
    }

    #[test]
    fn test_cannot_start_twice() {
        let s = RuntimeSupervisor::new(3, 60);
        assert!(s.start().is_ok());
        assert!(s.start().is_err());
    }

    #[test]
    fn test_cannot_stop_when_idle() {
        let s = RuntimeSupervisor::new(3, 60);
        assert!(s.stop().is_err());
    }

    #[test]
    fn test_restart() {
        let s = RuntimeSupervisor::new(3, 60);
        assert!(s.start().is_ok());
        assert!(s.restart().is_ok());
        assert_eq!(s.get_status(), RuntimeStatus::Running);
    }

    #[test]
    fn test_mark_unhealthy() {
        let s = RuntimeSupervisor::new(3, 60);
        s.start().unwrap();
        assert!(s.is_healthy());
        s.mark_unhealthy();
        assert!(!s.is_healthy());
        s.mark_healthy();
        assert!(s.is_healthy());
    }

    #[test]
    fn test_restart_from_idle() {
        let s = RuntimeSupervisor::new(3, 60);
        assert!(s.restart().is_ok(), "should allow restart from idle");
        assert_eq!(s.get_status(), RuntimeStatus::Running);
    }

    #[test]
    fn test_mark_error() {
        let s = RuntimeSupervisor::new(3, 60);
        s.start().unwrap();
        s.mark_error();
        assert_eq!(s.get_status(), RuntimeStatus::Error);
        assert!(!s.is_healthy());
    }
}
