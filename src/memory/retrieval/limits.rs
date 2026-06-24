/// 检索限制策略
#[derive(Debug, Clone)]
pub struct RetrievalLimits {
    /// 最大返回结果数
    pub max_results: usize,
    /// 最小相似度阈值
    pub min_similarity: f64,
    /// 最大记忆年龄（秒），None 表示不限制
    pub max_age_secs: Option<i64>,
    /// 是否排除已删除记忆
    pub exclude_deleted: bool,
}

impl Default for RetrievalLimits {
    fn default() -> Self {
        Self {
            max_results: 20,
            min_similarity: 0.1,
            max_age_secs: None,
            exclude_deleted: true,
        }
    }
}

/// 重排序候选文本最小长度限制（与 Python limits.py 一致）
pub const MIN_RERANK_CANDIDATE_MAX_CHARS: usize = 20;

/// 重排序提示词最小总预算（与 Python limits.py 一致）
pub const MIN_RERANK_TOTAL_PROMPT_BUDGET: usize = 200;
