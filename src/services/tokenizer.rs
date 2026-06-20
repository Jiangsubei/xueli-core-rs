use jieba_rs::Jieba;

use crate::traits::tokenizer::{TokenizedText, Tokenizer};

/// 基于 jieba-rs 的默认中文分词器
#[derive(Debug, Clone)]
pub struct JiebaTokenizer {
    jieba: Jieba,
}

impl JiebaTokenizer {
    /// 创建新的 jieba 分词器
    pub fn new() -> Self {
        Self {
            jieba: Jieba::new(),
        }
    }
}

impl Default for JiebaTokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Tokenizer for JiebaTokenizer {
    fn tokenize(&self, text: &str) -> TokenizedText {
        let tokens: Vec<String> = self
            .jieba
            .cut(text, true)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        TokenizedText {
            tokens,
            original: text.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jieba_tokenizer_basic() {
        let tokenizer = JiebaTokenizer::new();
        let result = tokenizer.tokenize("我爱北京天安门");
        assert!(!result.tokens.is_empty());
        assert_eq!(result.original, "我爱北京天安门");
    }

    #[test]
    fn test_jieba_tokenizer_for_search() {
        let tokenizer = JiebaTokenizer::new();
        let tokens = tokenizer.tokenize_for_search("用户喜欢喝咖啡");
        assert!(!tokens.is_empty());
        assert!(tokens.iter().any(|t| t.contains("咖啡")));
    }
}
