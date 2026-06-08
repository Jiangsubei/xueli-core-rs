use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use crate::core::types::MemoryItem;

use super::bm25_index::BM25Index;
use super::vector_index::VectorIndex;

/// 检索配置 — 两阶段检索的所有可调参数
#[derive(Debug, Clone)]
pub struct RetrievalConfig {
    pub bm25_top_k: usize,
    pub bm25_min_score: f64,
    pub local_bm25_weight: f64,
    pub local_importance_weight: f64,
    pub local_mention_weight: f64,
    pub local_recency_weight: f64,
    pub local_scene_weight: f64,
    pub vector_weight: f64,
    pub scene_same_group_weight: f64,
    pub scene_same_type_weight: f64,
    pub scene_same_user_weight: f64,
    pub archive_penalty_base: f64,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            bm25_top_k: 100,
            bm25_min_score: 0.0,
            local_bm25_weight: 1.0,
            local_importance_weight: 0.35,
            local_mention_weight: 0.2,
            local_recency_weight: 0.15,
            local_scene_weight: 0.3,
            vector_weight: 0.4,
            scene_same_group_weight: 1.5,
            scene_same_type_weight: 1.0,
            scene_same_user_weight: 0.8,
            archive_penalty_base: 0.5,
        }
    }
}

/// 检索请求上下文
#[derive(Debug, Clone)]
pub struct RetrievalContext {
    pub user_id: String,
    pub group_id: String,
    pub hour_of_day: i32,
}

impl Default for RetrievalContext {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            group_id: String::new(),
            hour_of_day: -1,
        }
    }
}

/// 检索结果条目
#[derive(Debug, Clone)]
pub struct RetrievedEntry {
    pub memory: MemoryItem,
    pub bm25_score: f64,
    pub local_score: f64,
    pub combined_score: f64,
    pub ranking_stage: String,
}

/// 两阶段记忆检索器
///
/// **Stage 1**：BM25 关键词检索 + 可选向量融合，粗筛 Top-K 候选。
/// **Stage 2**：本地多因子排序（重要性、提及次数、场景关联、新鲜度衰减等），输出最终 Top-K。
pub struct TwoStageRetriever {
    bm25_index: Arc<Mutex<BM25Index>>,
    vector_index: Option<Arc<Mutex<VectorIndex>>>,
    config: RetrievalConfig,
    memory_map: Arc<Mutex<HashMap<String, MemoryItem>>>,
}

impl TwoStageRetriever {
    pub fn new(config: RetrievalConfig, vector_index: Option<Arc<Mutex<VectorIndex>>>) -> Self {
        Self {
            bm25_index: Arc::new(Mutex::new(BM25Index::default())),
            vector_index,
            config,
            memory_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 添加记忆到索引
    pub fn add_memory(&self, memory: &MemoryItem) {
        let doc_id = memory.id.clone();
        let content = memory.content.clone();

        {
            let mut memory_map = self.memory_map.lock().expect("memory_map lock");
            memory_map.insert(doc_id.clone(), memory.clone());
        }

        {
            let mut bm25 = self.bm25_index.lock().expect("bm25 lock");
            bm25.add(doc_id.clone(), &content);
        }

        if let Some(ref vi) = self.vector_index {
            let mut vec = vi.lock().expect("vector lock");
            vec.add(doc_id, &content);
        }
    }

    /// 移除记忆从索引
    pub fn remove_memory(&self, doc_id: &str) {
        {
            let mut memory_map = self.memory_map.lock().expect("memory_map lock");
            memory_map.remove(doc_id);
        }
        {
            let mut bm25 = self.bm25_index.lock().expect("bm25 lock");
            bm25.remove_document(doc_id);
        }
        if let Some(ref vi) = self.vector_index {
            let mut vec = vi.lock().expect("vector lock");
            vec.remove_document(doc_id);
        }
    }

    /// 批量添加记忆
    pub fn add_memories_batch(&self, memories: &[MemoryItem]) {
        for mem in memories {
            self.add_memory(mem);
        }
    }

    /// 两阶段检索
    ///
    /// - `user_id`：请求用户 ID（用于过滤）
    /// - `query`：检索查询文本
    /// - `top_k`：最终返回结果数
    /// - `context`：检索上下文（场景、时间等）
    pub fn search(
        &self,
        user_id: &str,
        query: &str,
        top_k: usize,
        context: Option<&RetrievalContext>,
    ) -> Vec<RetrievedEntry> {
        let recall_k = self.config.bm25_top_k;

        let bm25_results = {
            let bm25 = self.bm25_index.lock().expect("bm25 lock");
            bm25.search(query, recall_k)
        };

        let vector_scores: HashMap<String, f64> = if let Some(ref vi) = self.vector_index {
            let vec = vi.lock().expect("vector lock");
            let hits = vec.search(query, recall_k);
            hits.into_iter().collect()
        } else {
            HashMap::new()
        };

        let fused_candidates = self.fuse_candidates(&bm25_results, &vector_scores);

        if fused_candidates.is_empty() {
            return vec![];
        }

        let memory_map = self.memory_map.lock().expect("memory_map lock");

        let candidate_list: Vec<(MemoryItem, f64)> = fused_candidates
            .into_iter()
            .filter_map(|(doc_id, fused_score)| {
                memory_map
                    .get(&doc_id)
                    .map(|mem| {
                        if user_id.is_empty() || mem.user_id == user_id {
                            Some((mem.clone(), fused_score))
                        } else {
                            None
                        }
                    })
                    .flatten()
            })
            .collect();

        drop(memory_map);

        let locally_ranked = self.apply_local_ranking(&candidate_list, context);

        locally_ranked
            .into_iter()
            .take(top_k)
            .map(|(mem, local_score)| {
                let bm25_score = bm25_results
                    .iter()
                    .find(|(id, _)| id == &mem.id)
                    .map(|(_, s)| *s)
                    .unwrap_or(0.0);
                RetrievedEntry {
                    combined_score: local_score,
                    bm25_score,
                    local_score,
                    ranking_stage: "local_prerank".to_string(),
                    memory: mem,
                }
            })
            .collect()
    }

    /// 快速检查：返回第一个超过阈值的匹配
    ///
    /// 用于判断是否有相关记忆（如 quick recall 场景）
    pub fn quick_check(&self, user_id: &str, query: &str, threshold: f64) -> Option<MemoryItem> {
        let bm25 = self.bm25_index.lock().expect("bm25 lock");
        let results = bm25.search(query, 1);

        if let Some((doc_id, score)) = results.first() {
            if *score < threshold || *score <= 0.0 {
                return None;
            }
            let memory_map = self.memory_map.lock().expect("memory_map lock");
            memory_map
                .get(doc_id)
                .map(|mem| {
                    if user_id.is_empty() || mem.user_id == user_id {
                        Some(mem.clone())
                    } else {
                        None
                    }
                })
                .flatten()
        } else {
            None
        }
    }

    /// 当前索引中的记忆数
    pub fn memory_count(&self) -> usize {
        let memory_map = self.memory_map.lock().expect("memory_map lock");
        memory_map.len()
    }

    fn fuse_candidates(
        &self,
        bm25_results: &[(String, f64)],
        vector_scores: &HashMap<String, f64>,
    ) -> Vec<(String, f64)> {
        let mut fused: HashMap<String, f64> = HashMap::new();

        for (doc_id, score) in bm25_results {
            fused.insert(doc_id.clone(), *score);
        }

        for (doc_id, vec_score) in vector_scores {
            let bm25_score = fused.get(doc_id).copied().unwrap_or(0.0);
            let fused_score = bm25_score * (1.0 - self.config.vector_weight)
                + vec_score * self.config.vector_weight;
            fused.insert(doc_id.clone(), fused_score);
        }

        let mut sorted: Vec<(String, f64)> = fused.into_iter().collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sorted
    }

    fn apply_local_ranking(
        &self,
        candidates: &[(MemoryItem, f64)],
        context: Option<&RetrievalContext>,
    ) -> Vec<(MemoryItem, f64)> {
        let mut scored: Vec<(MemoryItem, f64)> = candidates
            .iter()
            .map(|(mem, bm25_score)| {
                let local = self.compute_local_score(mem, *bm25_score, context);
                (mem.clone(), local)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }

    fn compute_local_score(
        &self,
        memory: &MemoryItem,
        bm25_score: f64,
        context: Option<&RetrievalContext>,
    ) -> f64 {
        let importance = self.normalize_importance_score(memory.importance);
        let mention_count = self.normalize_mention_score(memory.access_count as f64);
        let normalized_bm25 = self.normalize_bm25_score(bm25_score);
        let scene_score = self.compute_scene_score(memory, context);
        let recency_score = self.compute_recency_score(memory);
        let time_of_day_score = self.compute_time_of_day_match(memory, context);

        let mut score = normalized_bm25 * self.config.local_bm25_weight
            + importance * self.config.local_importance_weight
            + mention_count * self.config.local_mention_weight
            + recency_score * self.config.local_recency_weight
            + scene_score * self.config.local_scene_weight
            + time_of_day_score * 0.1;

        if self.is_archived(memory) {
            let archive_penalty = self.compute_archive_penalty(memory);
            score *= archive_penalty;
        }

        score
    }

    fn is_archived(&self, _memory: &MemoryItem) -> bool {
        false
    }

    fn compute_archive_penalty(&self, _memory: &MemoryItem) -> f64 {
        self.config.archive_penalty_base
    }

    fn compute_scene_score(&self, memory: &MemoryItem, context: Option<&RetrievalContext>) -> f64 {
        let ctx = match context {
            Some(c) => c,
            None => return 0.0,
        };

        if ctx.user_id.is_empty() && ctx.group_id.is_empty() {
            return 0.0;
        }

        let mut score = 0.0;

        if !ctx.user_id.is_empty() && memory.user_id == ctx.user_id {
            score += self.config.scene_same_user_weight;
        }

        score
    }

    fn compute_recency_score(&self, memory: &MemoryItem) -> f64 {
        let now = chrono::Utc::now().timestamp() as f64;
        let then = memory.last_accessed_at.timestamp() as f64;
        let age_seconds = (now - then).max(0.0);
        let age_days = age_seconds / 86400.0;
        (1.0 - (age_days / 30.0).min(1.0)).max(0.0)
    }

    fn compute_time_of_day_match(
        &self,
        memory: &MemoryItem,
        context: Option<&RetrievalContext>,
    ) -> f64 {
        let ctx = match context {
            Some(c) => c,
            None => return 0.0,
        };

        if ctx.hour_of_day < 0 {
            return 0.0;
        }

        let mem_hour = memory
            .created_at
            .format("%H")
            .to_string()
            .parse::<i32>()
            .unwrap_or(-1);
        if mem_hour / 6 == ctx.hour_of_day / 6 {
            return 0.15;
        }
        if (mem_hour - ctx.hour_of_day).abs() <= 6 {
            return 0.05;
        }
        0.0
    }

    fn normalize_bm25_score(&self, bm25_score: f64) -> f64 {
        let score = bm25_score.max(0.0);
        if score > 0.0 {
            score / (score + 3.0)
        } else {
            0.0
        }
    }

    fn normalize_importance_score(&self, importance: f64) -> f64 {
        let bounded = importance.clamp(0.0, 5.0);
        bounded / 5.0
    }

    fn normalize_mention_score(&self, mention_count: f64) -> f64 {
        let count = mention_count.max(0.0);
        if count > 0.0 {
            count / (count + 2.0)
        } else {
            0.0
        }
    }
}

/// 抑制注册表 — 防止同一 memory_id 被反复抑制（抑制风暴防护）
pub struct SuppressionRegistry {
    entries: Mutex<HashMap<String, usize>>,
    cooldown_interval: usize,
    retrieval_count: AtomicUsize,
}

impl SuppressionRegistry {
    pub fn new(cooldown: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            cooldown_interval: cooldown,
            retrieval_count: AtomicUsize::new(0),
        }
    }

    /// Return true if this (mem_id, query_hash) should be suppressed
    pub fn should_suppress(&self, mem_id: &str, query_hash: &str) -> bool {
        let count = self.retrieval_count.fetch_add(1, Ordering::SeqCst);
        let key = format!("{}:{}", mem_id, query_hash);
        let mut entries = self.entries.lock().unwrap();

        if count % self.cooldown_interval == 0 && count > 0 {
            entries.clear();
        }

        let current = entries.entry(key).or_insert(0);
        if *current >= 3 {
            return false;
        }
        *current += 1;
        true
    }

    pub fn reset(&self) {
        self.entries.lock().unwrap().clear();
        self.retrieval_count.store(0, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::MemoryType;

    fn make_memory(id: &str, user_id: &str, content: &str, importance: f64) -> MemoryItem {
        MemoryItem {
            id: id.to_string(),
            user_id: user_id.to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Fact,
            importance,
            created_at: chrono::Utc::now(),
            last_accessed_at: chrono::Utc::now(),
            access_count: 1,
        }
    }

    #[test]
    fn test_add_and_search() {
        let config = RetrievalConfig::default();
        let retriever = TwoStageRetriever::new(config, None);

        retriever.add_memory(&make_memory("m1", "u1", "用户喜欢咖啡", 0.8));
        retriever.add_memory(&make_memory("m2", "u1", "用户住在北京", 0.6));
        retriever.add_memory(&make_memory("m3", "u2", "用户喜欢苹果", 0.9));

        let results = retriever.search("u1", "咖啡", 5, None);
        assert!(!results.is_empty());
        assert_eq!(results[0].memory.id, "m1");
    }

    #[test]
    fn test_search_empty_index() {
        let config = RetrievalConfig::default();
        let retriever = TwoStageRetriever::new(config, None);

        let results = retriever.search("u1", "任何查询", 5, None);
        assert!(results.is_empty());
    }

    #[test]
    fn test_quick_check() {
        let config = RetrievalConfig::default();
        let retriever = TwoStageRetriever::new(config, None);
        retriever.add_memory(&make_memory("m1", "u1", "用户喜欢咖啡", 0.8));

        let found = retriever.quick_check("u1", "咖啡", 0.0);
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "m1");

        let not_found = retriever.quick_check("u1", "xyz123", 0.0);
        assert!(not_found.is_none());
    }

    #[test]
    fn test_quick_check_threshold() {
        let config = RetrievalConfig::default();
        let retriever = TwoStageRetriever::new(config, None);

        retriever.add_memory(&make_memory("m1", "u1", "用户喜欢咖啡", 0.8));

        let found = retriever.quick_check("u1", "咖啡", 0.1);
        assert!(found.is_some());

        let not_found = retriever.quick_check("u1", "咖啡", 9999.0);
        assert!(not_found.is_none());
    }

    #[test]
    fn test_remove_memory() {
        let config = RetrievalConfig::default();
        let retriever = TwoStageRetriever::new(config, None);

        retriever.add_memory(&make_memory("m1", "u1", "用户喜欢咖啡", 0.8));
        assert_eq!(retriever.memory_count(), 1);

        retriever.remove_memory("m1");
        assert_eq!(retriever.memory_count(), 0);

        let results = retriever.search("u1", "咖啡", 5, None);
        assert!(results.is_empty());
    }

    #[test]
    fn test_user_id_filtering() {
        let config = RetrievalConfig::default();
        let retriever = TwoStageRetriever::new(config, None);

        retriever.add_memory(&make_memory("m1", "u1", "用户喜欢咖啡", 0.8));
        retriever.add_memory(&make_memory("m2", "u2", "用户喜欢苹果", 0.9));

        let results_u1 = retriever.search("u1", "用户喜欢", 5, None);
        assert_eq!(results_u1.len(), 1);
        assert_eq!(results_u1[0].memory.id, "m1");

        let results_u2 = retriever.search("u2", "用户喜欢", 5, None);
        assert_eq!(results_u2.len(), 1);
        assert_eq!(results_u2[0].memory.id, "m2");

        let results_all = retriever.search("", "用户喜欢", 10, None);
        assert_eq!(results_all.len(), 2);
    }

    #[test]
    fn test_local_ranking_order() {
        let config = RetrievalConfig::default();
        let retriever = TwoStageRetriever::new(config, None);

        retriever.add_memory(&make_memory("m_low", "u1", "咖啡可以提神", 0.2));
        retriever.add_memory(&make_memory(
            "m_high",
            "u1",
            "咖啡非常好喝用户很喜欢每天都要喝",
            0.9,
        ));

        let results = retriever.search("u1", "咖啡", 5, None);
        assert!(!results.is_empty());
        assert_eq!(results[0].memory.id, "m_high");
    }

    #[test]
    fn test_context_scene_scoring() {
        let config = RetrievalConfig::default();
        let retriever = TwoStageRetriever::new(config, None);

        retriever.add_memory(&make_memory("m1", "u1", "用户喜欢咖啡", 0.5));

        let ctx = RetrievalContext {
            user_id: "u1".to_string(),
            group_id: String::new(),
            hour_of_day: -1,
        };

        let results_with_ctx = retriever.search("u1", "咖啡", 5, Some(&ctx));
        assert!(!results_with_ctx.is_empty());

        let results_no_ctx = retriever.search("u1", "咖啡", 5, None);
        assert!(!results_no_ctx.is_empty());
    }

    #[test]
    fn test_suppression_basic_within_limit() {
        let registry = SuppressionRegistry::new(100);
        assert!(registry.should_suppress("m1", "q1"));
        assert!(registry.should_suppress("m1", "q1"));
        assert!(registry.should_suppress("m1", "q1"));
        assert!(!registry.should_suppress("m1", "q1"));
    }

    #[test]
    fn test_suppression_cooldown_reset() {
        let registry = SuppressionRegistry::new(3);
        assert!(registry.should_suppress("m1", "q1"));
        assert!(registry.should_suppress("m1", "q1"));
        registry.should_suppress("m2", "q2"); // 3rd call triggers cooldown clear
        assert!(registry.should_suppress("m1", "q1"));
    }

    #[test]
    fn test_suppression_independent_keys() {
        let registry = SuppressionRegistry::new(100);
        assert!(registry.should_suppress("m1", "q1"));
        assert!(registry.should_suppress("m2", "q2"));
        assert!(registry.should_suppress("m1", "q2"));
    }
}
