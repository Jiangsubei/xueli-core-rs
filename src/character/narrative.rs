use chrono::{DateTime, Utc};

/// 叙事线 — 追踪长期相处中的叙事脉络
#[derive(Debug, Clone)]
pub struct NarrativeThread {
    pub id: String,
    pub user_id: String,
    pub theme: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub events: Vec<NarrativeEvent>,
}

/// 叙事事件
#[derive(Debug, Clone)]
pub struct NarrativeEvent {
    pub timestamp: DateTime<Utc>,
    pub description: String,
    pub significance: f64,
}

impl NarrativeThread {
    pub fn new(user_id: &str, theme: &str) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: user_id.to_string(),
            theme: theme.to_string(),
            created_at: now,
            updated_at: now,
            events: Vec::new(),
        }
    }
}
