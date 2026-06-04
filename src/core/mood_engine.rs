use crate::core::types::MoodState;

/// LLM 提供的逐轮情绪增量
#[derive(Debug, Clone, Default)]
pub struct MoodDeltas {
    pub valence_delta: Option<f64>,
    pub energy_delta: Option<f64>,
    pub arousal_delta: Option<f64>,
}

/// 情绪引擎 — 多维情绪状态管理
///
/// 状态由 LLM 提供的增量（通过 `apply_deltas()`）驱动。
/// 包含自发衰减、硬钳制、边界缩放、夜间恢复等机制。
/// 对应 Python 版 `xueli/src/core/mood_engine.py`
pub struct MoodEngine {
    enabled: bool,
    #[allow(dead_code)]
    volatility: f64,
    #[allow(dead_code)]
    independence_ratio: f64,
    energy_decay_per_turn: f64,
    #[allow(dead_code)]
    energy_recovery_night: f64,
    valence_decay_rate: f64,
    recovery_rate: f64,
    show_in_reply: bool,
    state: Option<MoodState>,
}

impl MoodEngine {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            volatility: 0.3,
            independence_ratio: 0.7,
            energy_decay_per_turn: 0.05,
            energy_recovery_night: 0.2,
            valence_decay_rate: 0.15,
            recovery_rate: 0.4,
            show_in_reply: false,
            state: if enabled {
                Some(MoodState::new())
            } else {
                None
            },
        }
    }

    pub fn current(&self) -> Option<&MoodState> {
        self.state.as_ref()
    }

    pub fn load(&mut self, state: Option<MoodState>) {
        self.state = state.or_else(|| {
            if self.enabled {
                Some(MoodState::new())
            } else {
                None
            }
        });
    }

    pub fn dump(&self) -> Option<MoodState> {
        self.state.clone()
    }

    /// 应用 LLM 提供的逐轮情绪增量
    ///
    /// 增量先被软钳制到 [-0.2, 0.2]，应用前先进行自发衰减
    /// （能量消耗 + valence 回归基线），应用后进行硬钳制。
    /// 边界缩放：越接近边界，增量作用越小。
    pub fn apply_deltas(&mut self, deltas: &MoodDeltas) {
        if !self.enabled {
            return;
        }
        let s = self.state.get_or_insert_with(MoodState::new);

        // 自发衰减：每轮能量消耗
        s.energy = (s.energy - self.energy_decay_per_turn).max(0.0);

        // 自发衰减：valence 向基线 0.0 回归（高唤醒时回归更慢）
        let arousal_gap = (s.arousal - 0.5).abs();
        let decay_rate = self.valence_decay_rate * s.energy;
        let valence_decay = -decay_rate * (s.valence - 0.0) * (1.0 - arousal_gap);
        s.valence += valence_decay;
        s.valence = s.valence.clamp(-1.0, 1.0);

        // 应用 LLM delta（软钳制 + 边界缩放）
        let clamp_delta = |d: f64| d.clamp(-0.2, 0.2);

        // valence
        if let Some(raw) = deltas.valence_delta {
            let raw = clamp_delta(raw);
            let current = s.valence;
            let room = 1.0 - current.abs();
            let room = room.max(0.0);
            let scale = (room * 1.5).clamp(0.3, 1.0);
            s.valence = (current + raw * scale).clamp(-1.0, 1.0);
        }

        // energy
        if let Some(raw) = deltas.energy_delta {
            let raw = clamp_delta(raw);
            let current = s.energy;
            let room = 1.0 - current;
            let room = room.max(0.0);
            let scale = (room * 1.5).clamp(0.3, 1.0);
            s.energy = (current + raw * scale).clamp(0.0, 1.0);
        }

        // arousal
        if let Some(raw) = deltas.arousal_delta {
            let raw = clamp_delta(raw);
            let current = s.arousal;
            let room = 1.0 - current;
            let room = room.max(0.0);
            let scale = (room * 1.5).clamp(0.3, 1.0);
            s.arousal = (current + raw * scale).clamp(0.0, 1.0);
        }

        s.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// 夜间恢复 — 能量缓慢回升
    ///
    /// 恢复量与当前能量水平相关：能量越低恢复越多。
    pub fn night_recovery(&mut self) {
        if !self.enabled {
            return;
        }
        if let Some(ref mut s) = self.state {
            let e = s.energy;
            s.energy = (e + self.recovery_rate * e * (1.0 - e)).min(1.0);
        }
    }

    /// 情绪修饰符回退 — 返回 (warmth, verbosity, initiative)
    ///
    /// 仅供回退使用；主要情绪驱动应由 LLM 原生处理。
    pub fn mood_modifier(&self) -> (f64, f64, f64) {
        let s = match &self.state {
            Some(s) if self.enabled => s,
            _ => return (0.0, 0.0, 0.0),
        };
        let warmth = s.valence * 0.3;
        let verbosity = (s.arousal - 0.5) * 0.4;
        let initiative = (s.energy - 0.5) * 0.4;
        (warmth, verbosity, initiative)
    }

    /// 情绪可见提示 — 仅在 show_in_reply 为 true 时返回非空
    pub fn mood_visible_hint(&self) -> String {
        if !self.show_in_reply || !self.enabled {
            return String::new();
        }
        let s = match &self.state {
            Some(s) => s,
            None => return String::new(),
        };
        if s.energy < 0.3 && s.valence < -0.2 {
            "今天有点累，但我还是想跟你聊聊".to_string()
        } else if s.valence > 0.5 && s.arousal > 0.6 {
            "今天心情不错".to_string()
        } else {
            String::new()
        }
    }
}

impl Default for MoodEngine {
    fn default() -> Self {
        Self::new(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_disabled() {
        let engine = MoodEngine::new(false);
        assert!(engine.current().is_none());
        assert_eq!(engine.mood_modifier(), (0.0, 0.0, 0.0));
    }

    #[test]
    fn test_new_enabled() {
        let engine = MoodEngine::new(true);
        let state = engine.current().unwrap();
        assert_eq!(state.valence, 0.0);
        assert_eq!(state.energy, 0.8);
        assert_eq!(state.arousal, 0.5);
    }

    #[test]
    fn test_apply_deltas_valence_positive() {
        let mut engine = MoodEngine::new(true);
        let deltas = MoodDeltas {
            valence_delta: Some(0.15),
            energy_delta: None,
            arousal_delta: None,
        };
        engine.apply_deltas(&deltas);
        let s = engine.current().unwrap();
        // 正向 delta 应使 valence 上升（考虑衰减和边界缩放后 > 0）
        assert!(
            s.valence > 0.0,
            "valence should increase, got {}",
            s.valence
        );
        // 硬钳制
        assert!(s.valence <= 1.0);
    }

    #[test]
    fn test_apply_deltas_energy_decay() {
        let mut engine = MoodEngine::new(true);
        let deltas = MoodDeltas::default();
        // 不提供任何 delta，energy 应因衰减而下降
        engine.apply_deltas(&deltas);
        let s = engine.current().unwrap();
        assert!(s.energy < 0.8, "energy should decay, got {}", s.energy);
    }

    #[test]
    fn test_apply_deltas_hard_clamp() {
        let mut engine = MoodEngine::new(true);
        // 设置超大 delta，应被软钳制到 0.2
        let deltas = MoodDeltas {
            valence_delta: Some(5.0),
            energy_delta: Some(-5.0),
            arousal_delta: Some(10.0),
        };
        engine.apply_deltas(&deltas);
        let s = engine.current().unwrap();
        assert!(
            s.valence >= -1.0 && s.valence <= 1.0,
            "valence out of bounds: {}",
            s.valence
        );
        assert!(
            s.energy >= 0.0 && s.energy <= 1.0,
            "energy out of bounds: {}",
            s.energy
        );
        assert!(
            s.arousal >= 0.0 && s.arousal <= 1.0,
            "arousal out of bounds: {}",
            s.arousal
        );
    }

    #[test]
    fn test_night_recovery() {
        let mut engine = MoodEngine::new(true);
        // 手动压低 energy
        if let Some(ref mut s) = engine.state {
            s.energy = 0.3;
        }
        engine.night_recovery();
        let s = engine.current().unwrap();
        assert!(s.energy > 0.3, "energy should recover, got {}", s.energy);
        assert!(s.energy <= 1.0);
    }

    #[test]
    fn test_night_recovery_disabled() {
        let mut engine = MoodEngine::new(false);
        engine.night_recovery();
        assert!(engine.current().is_none());
    }

    #[test]
    fn test_mood_visible_hint_disabled() {
        let engine = MoodEngine::new(true);
        assert_eq!(engine.mood_visible_hint(), "");
    }

    #[test]
    fn test_mood_visible_hint_tired() {
        let mut engine = MoodEngine::new(true);
        engine.show_in_reply = true;
        if let Some(ref mut s) = engine.state {
            s.energy = 0.2;
            s.valence = -0.5;
        }
        let hint = engine.mood_visible_hint();
        assert!(hint.contains("有点累"));
    }

    #[test]
    fn test_mood_visible_hint_happy() {
        let mut engine = MoodEngine::new(true);
        engine.show_in_reply = true;
        if let Some(ref mut s) = engine.state {
            s.valence = 0.7;
            s.arousal = 0.8;
        }
        let hint = engine.mood_visible_hint();
        assert!(hint.contains("心情不错"));
    }

    #[test]
    fn test_boundary_scaling_near_limit() {
        let mut engine = MoodEngine::new(true);
        // 将 valence 推到接近上限
        if let Some(ref mut s) = engine.state {
            s.valence = 0.9;
        }
        let deltas = MoodDeltas {
            valence_delta: Some(0.2),
            energy_delta: None,
            arousal_delta: None,
        };
        engine.apply_deltas(&deltas);
        let s = engine.current().unwrap();
        assert!(s.valence <= 1.0);
    }

    #[test]
    fn test_load_and_dump() {
        let mut engine = MoodEngine::new(true);
        let mut state = MoodState::new();
        state.valence = 0.5;
        state.energy = 0.6;
        engine.load(Some(state));
        let loaded = engine.current().unwrap();
        assert_eq!(loaded.valence, 0.5);
        assert_eq!(loaded.energy, 0.6);

        let dumped = engine.dump().unwrap();
        assert_eq!(dumped.valence, 0.5);
    }
}
