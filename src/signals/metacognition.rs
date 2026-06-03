/// 元认知信号
#[derive(Debug, Clone)]
pub struct Metacognition;

impl Metacognition {
    pub fn new() -> Self {
        Self
    }

    /// 判断用户消息是否为"元问题"（关于 bot 自身的提问）
    pub fn is_meta_question(&self, _text: &str) -> bool {
        // TODO: 实现元认知检测
        false
    }
}

impl Default for Metacognition {
    fn default() -> Self {
        Self::new()
    }
}