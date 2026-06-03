/// 分词结果
#[derive(Debug, Clone)]
pub struct TokenizedText {
    pub tokens: Vec<String>,
    pub original: String,
}

/// 分词器 trait — 默认 jieba 中文分词，支持下游替换
pub trait Tokenizer: Send + Sync {
    /// 分词
    fn tokenize(&self, text: &str) -> TokenizedText;

    /// 按搜索模式分词（用于 BM25 索引等场景）
    fn tokenize_for_search(&self, text: &str) -> Vec<String> {
        self.tokenize(text).tokens
    }
}