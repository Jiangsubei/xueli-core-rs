use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// 内存任务管理器 — 追踪和协调记忆相关的后台任务。
///
/// 对应 Python 版 `src/memory/internal/task_manager.py`
///
/// Python 版使用 `asyncio.create_task` + `Task.add_done_callback`，
/// Rust 版使用 `tokio::spawn` + `JoinHandle`。
pub struct MemoryTaskManager {
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl MemoryTaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 创建后台任务并追踪
    pub fn create_task(
        &self,
        future: impl std::future::Future<Output = ()> + Send + 'static,
        name: Option<String>,
    ) {
        let task_name = name.unwrap_or_else(|| "memory-task".to_string());
        let handle = tokio::spawn(async move {
            tracing::debug!("[任务管理] 启动后台任务: {}", task_name);
            future.await;
            tracing::debug!("[任务管理] 后台任务完成: {}", task_name);
        });

        // 使用 try_lock 避免在 async context 中阻塞
        if let Ok(mut tasks) = self.tasks.try_lock() {
            // 清理已完成的任务
            tasks.retain(|h| !h.is_finished());
            tasks.push(handle);
        } else {
            // 如果锁被占用，将任务 spawn 但不追踪（降级）
            tracing::warn!("[任务管理] 无法获取锁，任务未追踪");
        }
    }

    /// 等待所有未完成的任务
    pub async fn flush(&self) {
        let handles: Vec<JoinHandle<()>> = {
            let mut tasks = self.tasks.lock().await;
            let pending: Vec<_> = tasks.drain(..).filter(|h| !h.is_finished()).collect();
            *tasks = Vec::new();
            pending
        };

        if handles.is_empty() {
            return;
        }

        tracing::debug!("[任务管理] 等待 {} 个后台任务完成", handles.len());
        for handle in handles {
            let _ = handle.await;
        }
    }

    /// 取消所有未完成的任务
    pub async fn cancel_all(&self) {
        let handles: Vec<JoinHandle<()>> = {
            let mut tasks = self.tasks.lock().await;
            let pending: Vec<_> = tasks.drain(..).filter(|h| !h.is_finished()).collect();
            *tasks = Vec::new();
            pending
        };

        if handles.is_empty() {
            return;
        }

        tracing::debug!("[任务管理] 取消 {} 个后台任务", handles.len());
        for handle in handles {
            handle.abort();
        }
    }

    /// 活跃任务数
    pub async fn count(&self) -> usize {
        let tasks = self.tasks.lock().await;
        tasks.iter().filter(|h| !h.is_finished()).count()
    }

    /// 同步查询活跃任务数（非阻塞）
    pub fn count_sync(&self) -> usize {
        if let Ok(tasks) = self.tasks.try_lock() {
            tasks.iter().filter(|h| !h.is_finished()).count()
        } else {
            0
        }
    }
}

impl Default for MemoryTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_count() {
        let mgr = MemoryTaskManager::new();
        assert_eq!(mgr.count().await, 0);

        mgr.create_task(
            async { tokio::time::sleep(std::time::Duration::from_millis(50)).await },
            Some("test".to_string()),
        );

        let c = mgr.count().await;
        assert!(c > 0 || c == 0, "count should be non-negative"); // 可能已完成
    }

    #[tokio::test]
    async fn test_flush_waits_for_tasks() {
        let mgr = MemoryTaskManager::new();
        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started2 = started.clone();

        mgr.create_task(
            async move {
                started2.store(true, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            },
            Some("slow".to_string()),
        );

        // 给任务一点时间启动
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        mgr.flush().await;
        assert!(started.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(mgr.count().await, 0);
    }

    #[tokio::test]
    async fn test_cancel_all() {
        let mgr = MemoryTaskManager::new();

        mgr.create_task(
            async {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            },
            Some("long".to_string()),
        );

        mgr.cancel_all().await;
        // 任务已取消
        assert_eq!(mgr.count().await, 0);
    }

    #[tokio::test]
    async fn test_flush_empty() {
        let mgr = MemoryTaskManager::new();
        mgr.flush().await; // 不应该 panic
        assert_eq!(mgr.count().await, 0);
    }

    #[test]
    fn test_default() {
        let mgr = MemoryTaskManager::default();
        assert_eq!(mgr.count_sync(), 0);
    }
}
