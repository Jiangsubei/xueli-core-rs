use chrono::{DateTime, Duration, Utc};

/// 获取当前 UTC 时间
pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

/// 格式化时间为 ISO 8601
pub fn format_iso(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

/// 人性化时间差
pub fn humanize_duration(dt: &DateTime<Utc>) -> String {
    let diff = Utc::now() - *dt;
    let secs = diff.num_seconds();

    if secs < 60 {
        format!("{}秒前", secs)
    } else if secs < 3600 {
        format!("{}分钟前", secs / 60)
    } else if secs < 86400 {
        format!("{}小时前", secs / 3600)
    } else {
        format!("{}天前", secs / 86400)
    }
}