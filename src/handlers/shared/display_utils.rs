// 图片占位与窗口显示文本格式化
// 对应 Python 版 xueli/src/handlers/shared/display_utils.py

use std::collections::HashMap;

/// 构建图片占位文本
///
/// 规则（与 Python 版保持一致）：
/// - 单张图片且有逐图描述：`[图片] {description}`
/// - 多张图片且有逐图描述：`[图片1] {d1}，[图片2] {d2}...`
/// - 有合并描述（单张/多张）：`[图片] {merged}`
/// - 全部识别失败：`[图片]未成功识别`
/// - 兜底：`[图片]` 或 `[图片 x{n}]`
pub fn format_image_placeholder(
    image_count: usize,
    per_image_descriptions: &[String],
    vision_failure_count: usize,
    merged_description: &str,
) -> String {
    let clean: Vec<&str> = per_image_descriptions
        .iter()
        .map(|d| d.trim())
        .filter(|d| !d.is_empty())
        .collect();

    if image_count == 1 && !clean.is_empty() {
        return format!("[图片] {}", clean[0]);
    }
    if image_count >= 2 && !clean.is_empty() {
        let parts: Vec<String> = clean
            .iter()
            .take(image_count)
            .enumerate()
            .map(|(i, d)| format!("[图片{}] {}", i + 1, d))
            .collect();
        return parts.join("，");
    }

    let merged = merged_description.trim();
    if !merged.is_empty() && image_count <= 1 {
        return format!("[图片] {}", merged);
    }
    if !merged.is_empty() && image_count >= 2 {
        return format!("[图片] {}", merged);
    }

    if vision_failure_count > 0 && vision_failure_count >= image_count.max(1) {
        return "[图片]未成功识别".to_string();
    }

    if image_count <= 1 {
        "[图片]".to_string()
    } else {
        format!("[图片 x{}]", image_count)
    }
}

/// 从消息项构建窗口显示文本
///
/// 接收类似 Python dict 的 HashMap，字段包括：
/// - display_text / text / raw_text — 文本内容
/// - raw_image_count / image_count — 图片数量
/// - raw_has_image — 是否有图片标记
/// - per_image_descriptions — 逐图描述列表
/// - merged_description — 合并描述
/// - vision_failure_count — 视觉识别失败数
pub fn window_display_text(item: &HashMap<String, serde_json::Value>) -> String {
    let text = item
        .get("display_text")
        .or_else(|| item.get("text"))
        .or_else(|| item.get("raw_text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let raw_image_count = item
        .get("raw_image_count")
        .or_else(|| item.get("image_count"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let has_image_indicator = item
        .get("raw_has_image")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || raw_image_count > 0;

    let per_image_descriptions: Vec<String> = item
        .get("per_image_descriptions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let merged_desc = item
        .get("merged_description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let vision_failure_count = item
        .get("vision_failure_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let mut image_parts: Vec<String> = Vec::new();
    if has_image_indicator {
        let placeholder = format_image_placeholder(
            raw_image_count,
            &per_image_descriptions,
            vision_failure_count,
            &merged_desc,
        );
        if raw_image_count == 1 && !per_image_descriptions.is_empty() {
            if !text.contains(&per_image_descriptions[0]) {
                image_parts.push(placeholder);
            }
        } else if raw_image_count >= 2 && !per_image_descriptions.is_empty() {
            if !per_image_descriptions.iter().any(|d| text.contains(d)) {
                image_parts.push(placeholder);
            }
        } else if !merged_desc.is_empty() {
            if !text.contains(&merged_desc) {
                image_parts.push(placeholder);
            }
        } else if !text.contains(&placeholder) {
            image_parts.push(placeholder);
        }
    }

    let image_text = image_parts.join("\n");

    if !text.is_empty() && text != "用户发送了空文本" {
        if !image_text.is_empty() {
            format!("{}\n{}", text, image_text)
        } else {
            text
        }
    } else if !image_text.is_empty() {
        image_text
    } else if has_image_indicator {
        format_image_placeholder(
            raw_image_count,
            &per_image_descriptions,
            vision_failure_count,
            &merged_desc,
        )
    } else {
        "用户发送了空文本".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_image_placeholder_single_with_desc() {
        let result = format_image_placeholder(1, &["一只猫".into()], 0, "");
        assert_eq!(result, "[图片] 一只猫");
    }

    #[test]
    fn test_format_image_placeholder_multi_with_desc() {
        let result = format_image_placeholder(2, &["猫".into(), "狗".into()], 0, "");
        assert_eq!(result, "[图片1] 猫，[图片2] 狗");
    }

    #[test]
    fn test_format_image_placeholder_merged() {
        let result = format_image_placeholder(1, &[], 0, "一只猫和一只狗");
        assert_eq!(result, "[图片] 一只猫和一只狗");
    }

    #[test]
    fn test_format_image_placeholder_failure() {
        let result = format_image_placeholder(1, &[], 1, "");
        assert_eq!(result, "[图片]未成功识别");
    }

    #[test]
    fn test_format_image_placeholder_fallback_single() {
        let result = format_image_placeholder(1, &[], 0, "");
        assert_eq!(result, "[图片]");
    }

    #[test]
    fn test_format_image_placeholder_fallback_multi() {
        let result = format_image_placeholder(3, &[], 0, "");
        assert_eq!(result, "[图片 x3]");
    }

    #[test]
    fn test_window_display_text_empty() {
        let item: HashMap<String, serde_json::Value> = HashMap::new();
        let result = window_display_text(&item);
        assert_eq!(result, "用户发送了空文本");
    }

    #[test]
    fn test_window_display_text_plain() {
        let mut item: HashMap<String, serde_json::Value> = HashMap::new();
        item.insert("text".into(), serde_json::json!("你好"));
        let result = window_display_text(&item);
        assert_eq!(result, "你好");
    }
}
