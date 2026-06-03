use rand::Rng;

use crate::core::types::MoodState;

/// 情绪引擎 — 管理 bot 的情绪状态变化
pub struct MoodEngine {
    current_mood: MoodState,
    stability: f64,
    time_since_last_change_secs: f64,
}

impl MoodEngine {
    pub fn new() -> Self {
        Self {
            current_mood: MoodState::Neutral,
            stability: 0.8,
            time_since_last_change_secs: 0.0,
        }
    }

    /// 获取当前情绪
    pub fn current(&self) -> &MoodState {
        &self.current_mood
    }

    /// 根据交互更新情绪
    pub fn update(&mut self, interaction_sentiment: f64, delta_secs: f64) {
        self.time_since_last_change_secs += delta_secs;
        let mut rng = rand::thread_rng();

        if rng.gen::<f64>() > self.stability {
            self.current_mood = match interaction_sentiment {
                s if s > 0.5 => MoodState::Happy,
                s if s < -0.3 => MoodState::Sad,
                s if s > 0.2 => MoodState::Playful,
                _ => MoodState::Neutral,
            };
            self.time_since_last_change_secs = 0.0;
        }
    }
}

impl Default for MoodEngine {
    fn default() -> Self {
        Self::new()
    }
}