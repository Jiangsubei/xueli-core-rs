/// 互动参与度信号
#[derive(Debug, Clone)]
pub struct Engagement;

impl Engagement {
    pub fn new() -> Self {
        Self
    }

    /// 估算消息的参与度分数 (0.0 - 1.0)
    pub fn score(&self, text: &str) -> f64 {
        let len = text.chars().count();
        let questions = text.matches('?').count() + text.matches('？').count();
        let score = (len as f64 / 200.0 + questions as f64 * 0.2).min(1.0);
        (score * 100.0).round() / 100.0
    }
}

impl Default for Engagement {
    fn default() -> Self {
        Self::new()
    }
}