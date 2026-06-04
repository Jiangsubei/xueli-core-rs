/// 运行时指标
#[derive(Debug, Clone)]
pub struct RuntimeMetrics {
    /// 收到的总消息数
    pub total_messages_received: u64,
    /// 发送的总回复数
    pub total_replies_sent: u64,
    /// 忽略的消息数
    pub total_ignored: u64,
    /// 错误计数
    pub error_count: u64,
    /// 平均响应延迟（毫秒）
    pub avg_response_latency_ms: f64,
    /// AI 调用总次数
    pub total_ai_calls: u64,
    /// AI 调用总 token 数
    pub total_tokens_used: u64,
}

impl Default for RuntimeMetrics {
    fn default() -> Self {
        Self {
            total_messages_received: 0,
            total_replies_sent: 0,
            total_ignored: 0,
            error_count: 0,
            avg_response_latency_ms: 0.0,
            total_ai_calls: 0,
            total_tokens_used: 0,
        }
    }
}

impl RuntimeMetrics {
    pub fn record_message_received(&mut self) {
        self.total_messages_received += 1;
    }

    pub fn record_reply_sent(&mut self, latency_ms: f64) {
        self.total_replies_sent += 1;
        // 指数移动平均更新
        let alpha = 0.1;
        self.avg_response_latency_ms =
            alpha * latency_ms + (1.0 - alpha) * self.avg_response_latency_ms;
    }

    pub fn record_ignored(&mut self) {
        self.total_ignored += 1;
    }

    pub fn record_error(&mut self) {
        self.error_count += 1;
    }

    pub fn record_ai_call(&mut self, tokens: u64) {
        self.total_ai_calls += 1;
        self.total_tokens_used += tokens;
    }
}
