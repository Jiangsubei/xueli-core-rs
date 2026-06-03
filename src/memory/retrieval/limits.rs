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