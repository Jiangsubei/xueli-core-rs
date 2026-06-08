pub mod card_service;
pub mod narrative;

const SCOPE_GROUP: &str = "group";
const SCOPE_PRIVATE: &str = "private";

pub(crate) fn build_scope_payload_path(storage_dir: &str, user_id: &str, suffix: &str) -> String {
    let normalized = user_id.trim();
    if normalized.is_empty() {
        return format!("{}/unknown_{}", storage_dir, suffix);
    }
    let parts: Vec<&str> = normalized.splitn(3, ':').collect();
    if parts.len() >= 3 {
        if parts[1] == SCOPE_GROUP {
            let platform = parts[0];
            let remainder = parts[2];
            if let Some(pos) = remainder.rfind(':') {
                let group_id = &remainder[..pos];
                let uid = &remainder[pos + 1..];
                if !uid.is_empty() {
                    return format!(
                        "{}/{}_group_{}_{}_{}",
                        storage_dir, platform, group_id, uid, suffix
                    );
                }
            }
            return format!(
                "{}/{}_group_{}_{}",
                storage_dir,
                platform,
                remainder.replace(':', "_"),
                suffix
            );
        }
        if parts[1] == SCOPE_PRIVATE {
            return format!(
                "{}/{}_private_{}_{}",
                storage_dir, parts[0], parts[2], suffix
            );
        }
    }
    format!("{}/{}_{}", storage_dir, normalized, suffix)
}

pub(crate) fn legacy_payload_path(storage_dir: &str, user_id: &str, suffix: &str) -> String {
    format!("{}/{}_{}", storage_dir, user_id, suffix)
}
