use chrono::{DateTime, Utc, Timelike};

/// 时间上下文信号
pub struct TemporalContext {
    pub current_time: DateTime<Utc>,
    pub session_start: DateTime<Utc>,
    pub timezone_offset_hours: i32,
}

impl TemporalContext {
    pub fn new(timezone_offset_hours: i32) -> Self {
        let now = Utc::now();
        Self {
            current_time: now,
            session_start: now,
            timezone_offset_hours,
        }
    }

    /// 会话持续秒数
    pub fn session_duration_secs(&self) -> i64 {
        (self.current_time - self.session_start).num_seconds()
    }

    /// 本地时间的小时数
    pub fn local_hour(&self) -> u32 {
        let hour = self.current_time.hour() as i32 + self.timezone_offset_hours;
        ((hour % 24 + 24) % 24) as u32
    }

    /// 是否为夜间
    pub fn is_night(&self) -> bool {
        let hour = self.local_hour();
        hour < 6 || hour >= 23
    }
}