/// 从结构化信号源追踪跨轮次元认知状态
///
/// 维护 per-user caution snapshot 滑动窗口，
/// 推导趋势信号和自我状态报告。所有信号来自已有结构化数据，
/// 不产生额外 LLM 调用。
///
/// 对应 Python 版 `xueli/src/handlers/signals/metacognition.py`
use std::collections::{HashMap, VecDeque};

/// 元认知快照
#[derive(Debug, Clone)]
pub struct MetacognitionSnapshot {
    pub timestamp: f64,
    pub caution_level: String,
    pub reason_count: usize,
    pub soft_uncertainty_count: usize,
    pub has_negative_feedback: bool,
    pub memory_available: bool,
}

/// 跨轮次元认知信号追踪器
pub struct MetacognitionMonitor {
    history: HashMap<String, VecDeque<MetacognitionSnapshot>>,
    max_history: usize,
}

impl MetacognitionMonitor {
    pub fn new(max_history: usize) -> Self {
        Self {
            history: HashMap::new(),
            max_history,
        }
    }

    /// 记录一轮快照
    pub fn record_snapshot(
        &mut self,
        user_id: &str,
        caution_level: &str,
        reason_count: usize,
        soft_uncertainty_count: usize,
        has_negative_feedback: bool,
        memory_available: bool,
    ) {
        let snapshot = MetacognitionSnapshot {
            timestamp: chrono::Utc::now().timestamp() as f64,
            caution_level: Self::normalize_caution_level(caution_level),
            reason_count,
            soft_uncertainty_count,
            has_negative_feedback,
            memory_available,
        };

        self.history
            .entry(user_id.to_string())
            .or_insert_with(|| VecDeque::with_capacity(self.max_history))
            .push_back(snapshot);

        // 保持滑动窗口大小
        if let Some(deque) = self.history.get_mut(user_id) {
            while deque.len() > self.max_history {
                deque.pop_front();
            }
        }
    }

    /// 计算趋势：rising / falling / stable
    pub fn get_trend(&self, user_id: &str, window: usize) -> String {
        let snapshots = match self.history.get(user_id) {
            Some(s) => s,
            None => return "stable".to_string(),
        };

        if snapshots.len() < window {
            return "stable".to_string();
        }

        let recent: Vec<&MetacognitionSnapshot> = snapshots
            .iter()
            .rev()
            .take(window)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let level_score = |level: &str| -> f64 {
            match level {
                "low" => 0.0,
                "medium" => 0.5,
                "high" => 1.0,
                _ => 0.5,
            }
        };

        let scores: Vec<f64> = recent
            .iter()
            .map(|s| level_score(&s.caution_level))
            .collect();
        let slope = if scores.len() >= 2 {
            (scores[scores.len() - 1] - scores[0]) / ((scores.len() - 1) as f64).max(1.0)
        } else {
            0.0
        };

        if slope > 0.15 && scores.last().copied().unwrap_or(0.0) >= 0.5 {
            "rising".to_string()
        } else if slope < -0.15 && scores.last().copied().unwrap_or(0.0) <= 0.5 {
            "falling".to_string()
        } else {
            "stable".to_string()
        }
    }

    /// 生成自我状态报告文本
    pub fn get_self_state_report(&self, user_id: &str) -> String {
        let snapshots = match self.history.get(user_id) {
            Some(s) => s,
            None => return String::new(),
        };

        if snapshots.len() < 3 {
            return String::new();
        }

        let recent: Vec<&MetacognitionSnapshot> = snapshots
            .iter()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let levels: Vec<&str> = recent.iter().map(|s| s.caution_level.as_str()).collect();
        let high_count = levels.iter().filter(|&&l| l == "high").count();

        if high_count >= 2 && levels.last() == Some(&"high") {
            return "近几轮回复不确定性持续偏高，建议保持谨慎".to_string();
        }

        if snapshots.len() >= 4 {
            let older = &snapshot_at(snapshots, snapshots.len().saturating_sub(4));
            if levels.last() == Some(&"low") && older.caution_level != "low" {
                return "不确定性已回落，可适度恢复正常回复策略".to_string();
            }
        }

        let negatives = recent.iter().filter(|s| s.has_negative_feedback).count();
        if negatives >= 2
            && recent
                .last()
                .map(|s| s.has_negative_feedback)
                .unwrap_or(false)
        {
            return "近期收到负面反馈，建议放低姿态、避免过度表达".to_string();
        }

        if snapshots.len() >= 2 {
            let len = snapshots.len();
            let pair = (
                snapshot_at(snapshots, len - 2),
                snapshot_at(snapshots, len - 1),
            );
            if pair.0.caution_level == "high" && pair.1.caution_level == "medium" {
                return "不确定性在改善中，可逐步恢复正常回复".to_string();
            }
        }

        String::new()
    }

    fn normalize_caution_level(level: &str) -> String {
        match level.to_lowercase().trim() {
            "low" => "low",
            "medium" => "medium",
            "high" => "high",
            _ => "low",
        }
        .to_string()
    }
}

fn snapshot_at(snapshots: &VecDeque<MetacognitionSnapshot>, idx: usize) -> &MetacognitionSnapshot {
    &snapshots[idx]
}

impl Default for MetacognitionMonitor {
    fn default() -> Self {
        Self::new(30)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_monitor() -> MetacognitionMonitor {
        MetacognitionMonitor::new(10)
    }

    #[test]
    fn test_record_and_basic_trend() {
        let mut m = make_monitor();
        // 全部 low
        for _ in 0..5 {
            m.record_snapshot("u1", "low", 0, 0, false, true);
        }
        assert_eq!(m.get_trend("u1", 3), "stable");
    }

    #[test]
    fn test_rising_trend() {
        let mut m = make_monitor();
        m.record_snapshot("u1", "low", 0, 0, false, true);
        m.record_snapshot("u1", "medium", 1, 0, false, true);
        m.record_snapshot("u1", "high", 2, 0, false, true);
        m.record_snapshot("u1", "high", 3, 0, false, true);
        assert_eq!(m.get_trend("u1", 3), "rising");
    }

    #[test]
    fn test_falling_trend() {
        let mut m = make_monitor();
        m.record_snapshot("u1", "high", 3, 0, false, true);
        m.record_snapshot("u1", "medium", 1, 0, false, true);
        m.record_snapshot("u1", "low", 0, 0, false, true);
        assert_eq!(m.get_trend("u1", 3), "falling");
    }

    #[test]
    fn test_self_state_report_high_caution() {
        let mut m = make_monitor();
        m.record_snapshot("u1", "high", 3, 0, false, true);
        m.record_snapshot("u1", "high", 2, 0, false, true);
        m.record_snapshot("u1", "high", 3, 0, false, true);
        let report = m.get_self_state_report("u1");
        assert!(report.contains("不确定性持续偏高"));
    }

    #[test]
    fn test_self_state_report_negative_feedback() {
        let mut m = make_monitor();
        m.record_snapshot("u1", "medium", 1, 0, true, true);
        m.record_snapshot("u1", "medium", 1, 0, true, true);
        m.record_snapshot("u1", "low", 0, 0, true, true);
        let report = m.get_self_state_report("u1");
        assert!(report.contains("负面反馈"));
    }

    #[test]
    fn test_self_state_report_empty_for_short_history() {
        let mut m = make_monitor();
        m.record_snapshot("u1", "low", 0, 0, false, true);
        m.record_snapshot("u1", "low", 0, 0, false, true);
        assert_eq!(m.get_self_state_report("u1"), "");
    }
}
