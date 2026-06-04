// 身份标签格式化工具
// 对应 Python 版 xueli/src/handlers/shared/identity_utils.py

/// 格式化用户身份标签：`{user_id}（{display_name}）`
///
/// - 如果 display_name 非空且与 user_id 不同，返回 `{user_id}（{display_name}）`
/// - 否则返回 `{user_id}`
/// - user_id 为空时兜底为 "unknown"
pub fn format_identity_label(user_id: &str, display_name: &str) -> String {
    let identifier = if user_id.trim().is_empty() {
        "unknown"
    } else {
        user_id.trim()
    };
    let name = display_name.trim();
    if !name.is_empty() && name != identifier {
        format!("{}（{}）", identifier, name)
    } else {
        identifier.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_label_with_display_name() {
        assert_eq!(format_identity_label("user123", "小明"), "user123（小明）");
    }

    #[test]
    fn test_label_same_name() {
        assert_eq!(format_identity_label("user123", "user123"), "user123");
    }

    #[test]
    fn test_label_empty_display_name() {
        assert_eq!(format_identity_label("user123", ""), "user123");
    }

    #[test]
    fn test_label_empty_user_id() {
        assert_eq!(format_identity_label("", "小明"), "unknown（小明）");
    }
}
