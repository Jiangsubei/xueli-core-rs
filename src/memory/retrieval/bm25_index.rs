use jieba_rs::Jieba;
use std::collections::{HashMap, HashSet};

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
    /// 是否脏（需要重建）
    dirty: bool,
}

impl BM25Index {
    pub fn new(k1: f64, b: f64) -> Self {
        Self {
            jieba: Jieba::new(),
            documents: Vec::new(),
            avg_doc_len: 0.0,
            k1,
            b,
            dirty: false,
        }
    }

    pub fn clear(&mut self) {
        self.documents.clear();
        self.avg_doc_len = 0.0;
    }

    pub fn remove_document(&mut self, doc_id: &str) {
        self.documents.retain(|(id, _)| id != doc_id);
        self.update_avg_len();
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
                let denominator =
                    tf + self.k1 * (1.0 - self.b + self.b * doc_len / self.avg_doc_len);

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

    /// 提取关键词：jieba-rs 分词 → 按 IDF 加权取 top-N
    pub fn extract_keywords(&self, text: &str, top_n: usize) -> Vec<String> {
        let tokens: Vec<String> = self
            .jieba
            .cut(text, true)
            .into_iter()
            .map(|s| s.to_string())
            .filter(|s| s.trim().len() > 1)
            .collect();

        if tokens.is_empty() {
            return Vec::new();
        }

        let n = self.documents.len() as f64;
        let mut tf_map: HashMap<&str, usize> = HashMap::new();
        for t in &tokens {
            *tf_map.entry(t.as_str()).or_insert(0) += 1;
        }

        let mut scored: Vec<(String, f64)> = tokens
            .iter()
            .filter(|t| {
                t.chars()
                    .any(|c| c.is_alphabetic() || ('\u{4e00}'..='\u{9fff}').contains(&c))
            })
            .map(|t| {
                let tf = *tf_map.get(t.as_str()).unwrap_or(&0) as f64;
                let df = self
                    .documents
                    .iter()
                    .filter(|(_, doc_tokens)| doc_tokens.iter().any(|dt| dt == t))
                    .count() as f64;
                let idf = if df > 0.0 && n > 0.0 {
                    ((n - df + 0.5) / (df + 0.5) + 1.0).ln().max(0.0)
                } else {
                    1.0
                };
                let score = tf * idf;
                (t.clone(), score)
            })
            .collect();

        // 去重按分数排序
        let mut seen: HashSet<String> = HashSet::new();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut result = Vec::new();
        for (kw, _) in scored {
            if seen.insert(kw.clone()) {
                result.push(kw);
                if result.len() >= top_n {
                    break;
                }
            }
        }
        result
    }

    /// 回退搜索：索引为空时使用 substring 匹配
    pub fn fallback_search(&self, query: &str, top_k: usize) -> Vec<(String, f64)> {
        let normalized = query.to_lowercase();
        let mut results: Vec<(String, f64)> = self
            .documents
            .iter()
            .filter_map(|(id, _tokens)| {
                let text = self.get_document_text(id);
                let text_lower = text.to_lowercase();
                if text_lower.contains(&normalized) {
                    // 基于子串匹配长度打分
                    let score = normalized.len() as f64 / text_lower.len().max(1) as f64;
                    Some((id.clone(), score))
                } else {
                    // 字符重叠度
                    let q_chars: HashSet<char> = normalized.chars().collect();
                    let t_chars: HashSet<char> = text_lower.chars().collect();
                    let overlap = q_chars.intersection(&t_chars).count();
                    if overlap > 0 {
                        let score = overlap as f64 / q_chars.len().max(1) as f64;
                        if score >= 0.12 {
                            return Some((id.clone(), score));
                        }
                    }
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }

    /// 获取文档的原始文本（通过重新拼接 token）
    fn get_document_text(&self, doc_id: &str) -> String {
        for (id, tokens) in &self.documents {
            if id == doc_id {
                return tokens.join("");
            }
        }
        String::new()
    }

    /// 获取索引统计信息
    pub fn get_stats(&self) -> BM25Stats {
        let total_tokens: usize = self.documents.iter().map(|(_, t)| t.len()).sum();
        let mut vocab: HashSet<String> = HashSet::new();
        for (_, tokens) in &self.documents {
            for t in tokens {
                vocab.insert(t.clone());
            }
        }
        BM25Stats {
            doc_count: self.documents.len(),
            avg_doc_len: self.avg_doc_len,
            total_tokens,
            vocab_size: vocab.len(),
        }
    }

    /// 标记为脏，下次查询时重建索引
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// 检查并清除脏标志（由调用方在重建后调用）
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// 检查是否脏
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// BM25 索引统计信息
#[derive(Debug, Clone)]
pub struct BM25Stats {
    pub doc_count: usize,
    pub avg_doc_len: f64,
    pub total_tokens: usize,
    pub vocab_size: usize,
}

impl Default for BM25Index {
    fn default() -> Self {
        Self::new(1.5, 0.75)
    }
}
