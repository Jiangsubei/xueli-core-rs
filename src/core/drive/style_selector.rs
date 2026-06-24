use std::collections::{HashMap, HashSet};

use crate::core::drive::models::DriveContext;

#[derive(Debug, Clone)]
struct DimensionState {
    value: f64,
    side: String,
}

impl Default for DimensionState {
    fn default() -> Self {
        Self {
            value: 0.0,
            side: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StyleSelector {
    alpha: f64,
    state: HashMap<String, HashMap<String, DimensionState>>,
}

impl StyleSelector {
    pub fn new(alpha: f64) -> Self {
        Self {
            alpha,
            state: HashMap::new(),
        }
    }

    pub fn select(
        &mut self,
        drive_context: Option<&DriveContext>,
        scope_key: &str,
        user_id: &str,
    ) -> HashSet<String> {
        let Some(drive_context) = drive_context else {
            return HashSet::new();
        };
        let key = Self::state_key(scope_key, user_id);
        let raw = Self::compute_raw_signals(drive_context);
        let mut moods: HashSet<String> = HashSet::new();
        let dimensions = self.state.entry(key).or_default();
        for (dimension, value) in raw {
            let state = dimensions
                .entry(dimension.clone())
                .or_insert_with(|| DimensionState {
                    value,
                    ..Default::default()
                });
            state.value = self.alpha * value + (1.0 - self.alpha) * state.value;
            let side = Self::hysteresis(&dimension, state.value, &state.side);
            state.side = side.clone();
            if !side.is_empty() {
                moods.insert(format!("mood.{}_{}", side, dimension));
            }
        }
        moods
    }

    pub fn reset(&mut self, scope_key: &str, user_id: &str) {
        let key = Self::state_key(scope_key, user_id);
        self.state.remove(&key);
    }

    fn state_key(scope_key: &str, user_id: &str) -> String {
        let scope = if scope_key.is_empty() { "global" } else { scope_key };
        let user = if user_id.is_empty() { "anonymous" } else { user_id };
        format!("{}:{}", scope, user)
    }

    fn compute_raw_signals(drive_context: &DriveContext) -> HashMap<String, f64> {
        let mut signals = HashMap::new();
        let pad = &drive_context.affective;
        signals.insert("warmth".to_string(), (pad.valence + 1.0) * 0.5);
        signals.insert("verbosity".to_string(), pad.arousal);
        let social_drive = drive_context
            .motivational
            .get("social_drive")
            .copied()
            .unwrap_or(0.5);
        signals.insert("initiative".to_string(), social_drive);
        signals
    }

    fn hysteresis(dimension: &str, value: f64, current_side: &str) -> String {
        let _ = dimension;
        let low_enter = 0.30;
        let low_exit = 0.40;
        let high_enter = 0.70;
        let high_exit = 0.60;

        if current_side == "low" {
            if value > low_exit {
                return String::new();
            }
            return "low".to_string();
        }
        if current_side == "high" {
            if value < high_exit {
                return String::new();
            }
            return "high".to_string();
        }
        if value < low_enter {
            return "low".to_string();
        }
        if value > high_enter {
            return "high".to_string();
        }
        String::new()
    }

    pub fn to_dict(&self) -> HashMap<String, HashMap<String, HashMap<String, f64>>> {
        let mut result = HashMap::new();
        for (key, dimensions) in &self.state {
            let mut dim_map = HashMap::new();
            for (dim, state) in dimensions {
                let mut inner = HashMap::new();
                inner.insert("value".to_string(), state.value);
                let side_code = match state.side.as_str() {
                    "low" => 1.0,
                    "high" => 2.0,
                    _ => 0.0,
                };
                inner.insert("side".to_string(), side_code);
                dim_map.insert(dim.clone(), inner);
            }
            result.insert(key.clone(), dim_map);
        }
        result
    }
}
