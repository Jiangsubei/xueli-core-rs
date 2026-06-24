/// 基于 char n-gram 的简单向量相似度索引
pub struct VectorIndex {
    /// 文档集合: (id, n-gram 频率向量)
    documents: Vec<(String, Vec<f64>)>,
    /// n-gram 大小
    n: usize,
    /// 词汇表大小
    vocab_size: usize,
}

impl VectorIndex {
    pub fn new(n: usize, vocab_size: usize) -> Self {
        Self {
            documents: Vec::new(),
            n,
            vocab_size,
        }
    }

    pub fn clear(&mut self) {
        self.documents.clear();
    }

    pub fn remove_document(&mut self, doc_id: &str) {
        self.documents.retain(|(id, _)| id != doc_id);
    }

    /// 添加文档
    pub fn add(&mut self, doc_id: String, text: &str) {
        let vec = self.text_to_vector(text);
        self.documents.push((doc_id, vec));
    }

    /// cosine 相似度搜索
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(String, f64)> {
        self.search_with_min_score(query, top_k, 0.0)
    }

    /// cosine 相似度搜索（带最低分数过滤）
    pub fn search_with_min_score(
        &self,
        query: &str,
        top_k: usize,
        min_score: f64,
    ) -> Vec<(String, f64)> {
        let query_vec = self.text_to_vector(query);
        let mut scores: Vec<(String, f64)> = self
            .documents
            .iter()
            .map(|(id, doc_vec)| {
                let score = cosine_similarity(&query_vec, doc_vec);
                (id.clone(), score)
            })
            .filter(|(_, score)| *score >= min_score)
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(top_k);
        scores
    }

    fn text_to_vector(&self, text: &str) -> Vec<f64> {
        let chars: Vec<char> = text.chars().collect();
        let mut vec = vec![0.0; self.vocab_size];
        let n_grams: Vec<String> = chars.windows(self.n).map(|w| w.iter().collect()).collect();

        for ng in &n_grams {
            let idx = ng.chars().fold(0usize, |acc, c| {
                acc.wrapping_mul(31).wrapping_add(c as usize)
            }) % self.vocab_size;
            vec[idx] += 1.0;
        }

        // L2 归一化
        let norm: f64 = vec.iter().map(|v| v * v).sum::<f64>().sqrt();
        if norm > 0.0 {
            vec.iter_mut().for_each(|v| *v /= norm);
        }

        vec
    }

    /// 获取索引中的文档数
    pub fn doc_count(&self) -> usize {
        self.documents.len()
    }
}

fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|v| v * v).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|v| v * v).sum::<f64>().sqrt();
    let denom = norm_a * norm_b;
    if denom > 0.0 {
        dot / denom
    } else {
        0.0
    }
}

impl Default for VectorIndex {
    fn default() -> Self {
        Self::new(2, 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-10);

        let c = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &c) - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_search_with_min_score() {
        let mut index = VectorIndex::default();
        index.add("d1".to_string(), "abc xyz");
        index.add("d2".to_string(), "完全不同的文本");
        let results = index.search_with_min_score("abc", 5, 0.5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "d1");
    }
}
