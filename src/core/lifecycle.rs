use tokio::task::JoinHandle;

/// 任务与资源生命周期管理
pub struct LifecycleManager {
    /// 后台任务句柄集合
    tasks: Vec<JoinHandle<()>>,
}

impl LifecycleManager {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
        }
    }

    /// 注册后台任务
    pub fn spawn<F>(&mut self, task: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(task);
        self.tasks.push(handle);
    }

    /// 取消所有后台任务
    pub async fn shutdown(&mut self) {
        for handle in self.tasks.drain(..) {
            handle.abort();
        }
    }
}

impl Default for LifecycleManager {
    fn default() -> Self {
        Self::new()
    }
}