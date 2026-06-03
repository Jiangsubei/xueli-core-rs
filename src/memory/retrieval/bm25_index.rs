use std::collections::HashMap;
use jieba_rs::Jieba;

/// BM25 文本检索索引
pub struct BM25Index {
    jieba: Jieba,
    /// 文档集合
    documents: Vec<(String, Vec<String>)>,
    /// 平均文档长度
    avg_doc_len: f64,
    /// BM25 参数 k1
    k1: f64,
    /// BM25 参数 b
    b: f64,
}

impl BM25Index {
    pub fn new(k1: f64, b: f64) -> Self {
        Self {
            jieba: Jieba::new(),
            documents: Vec::new(),
            avg_doc_len: 0.0,
            k1,
            b,
        }
    }

    /// 添加文档到索引
    pub fn add(&mut self, doc_id: String, text: &str) {
        let tokens: Vec<String> = self
            .jieba
            .cut(text, true)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        self.documents.push((doc_id, tokens));
        self.update_avg_len();
    }

    /// 搜索
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(String, f64)> {
        let query_tokens: Vec<String> = self
            .jieba
            .cut(query, true)
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        let mut scores: Vec<(String, f64)> = self
            .documents
            .iter()
            .map(|(id, doc_tokens)| {
                let score = self.bm25_score(&query_tokens, doc_tokens);
                (id.clone(), score)
            })
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(top_k);
        scores
    }

    fn bm25_score(&self, query_tokens: &[String], doc_tokens: &[String]) -> f64 {
        let doc_len = doc_tokens.len() as f64;
        let n = self.documents.len() as f64;

        let mut tf_map: HashMap<&str, usize> = HashMap::new();
        for t in doc_tokens {
            *tf_map.entry(t).or_insert(0) += 1;
        }

        query_tokens
            .iter()
            .map(|qt| {
                let tf = *tf_map.get(qt.as_str()).unwrap_or(&0) as f64;
                if tf == 0.0 {
                    return 0.0;
                }

                let df = self
                    .documents
                    .iter()
                    .filter(|(_, tokens)| tokens.contains(qt))
                    .count() as f64;

                let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln().max(0.0);
                let numerator = tf * (self.k1 + 1.0);
                let denominator = tf + self.k1 * (1.0 - self.b + self.b * doc_len / self.avg_doc_len);

                idf * numerator / denominator
            })
            .sum()
    }

    fn update_avg_len(&mut self) {
        let total: usize = self.documents.iter().map(|(_, t)| t.len()).sum();
        self.avg_doc_len = if self.documents.is_empty() {
            1.0
        } else {
            total as f64 / self.documents.len() as f64
        };
    }
}

impl Default for BM25Index {
    fn default() -> Self {
        Self::new(1.5, 0.75)
    }
}