/// ID 生成工具
pub fn generate_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 生成短 ID（8 字符）
pub fn generate_short_id() -> String {
    let id = uuid::Uuid::new_v4().to_string();
    id.chars().take(8).collect()
}
