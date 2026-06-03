/// 混合评分器 — 融合 BM25 和向量检索结果
pub struct HybridScorer {
    /// BM25 权重
    bm25_weight: f64,
    /// 向量权重
    vector_weight: f64,
}

/// 混合得分项
#[derive(Debug, Clone)]
pub struct ScoredItem {
    pub id: String,
    pub bm25_score: f64,
    pub vector_score: f64,
    pub hybrid_score: f64,
}

impl HybridScorer {
    pub fn new(bm25_weight: f64, vector_weight: f64) -> Self {
        Self {
            bm25_weight,
            vector_weight,
        }
    }

    /// 融合两个来源的分数
    pub fn fuse(
        &self,
        bm25_results: &[(String, f64)],
        vector_results: &[(String, f64)],
        top_k: usize,
    ) -> Vec<ScoredItem> {
        use std::collections::HashMap;

        let mut scores: HashMap<String, (f64, f64)> = HashMap::new();

        for (id, score) in bm25_results {
            scores.entry(id.clone()).or_insert((0.0, 0.0)).0 = *score;
        }

        for (id, score) in vector_results {
            scores.entry(id.clone()).or_insert((0.0, 0.0)).1 = *score;
        }

        let mut items: Vec<ScoredItem> = scores
            .into_iter()
            .map(|(id, (bm25, vec))| ScoredItem {
                hybrid_score: self.bm25_weight * bm25 + self.vector_weight * vec,
                bm25_score: bm25,
                vector_score: vec,
                id,
            })
            .collect();

        items.sort_by(|a, b| {
            b.hybrid_score
                .partial_cmp(&a.hybrid_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        items.truncate(top_k);
        items
    }
}

impl Default for HybridScorer {
    fn default() -> Self {
        Self::new(0.6, 0.4)
    }
}