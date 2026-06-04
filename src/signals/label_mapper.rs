/// 枚举/数值信号 → 自然语言中文标签的纯函数映射
///
/// 对应 Python 版 `xueli/src/handlers/signals/label_mapper.py`
use std::collections::HashMap;

/// 将心情状态映射为中文标签
pub fn mood_state_label(valence: f64, arousal: f64, energy: f64) -> String {
    format!(
        "{}，{}，{}",
        valence_label(valence),
        arousal_label(arousal),
        energy_label(energy),
    )
}

fn valence_label(v: f64) -> &'static str {
    if v >= 0.3 {
        "积极"
    } else if v >= -0.3 {
        "中性"
    } else {
        "消沉"
    }
}

fn arousal_label(a: f64) -> &'static str {
    if a >= 0.6 {
        "兴奋"
    } else if a >= 0.4 {
        "平稳"
    } else {
        "低沉"
    }
}

fn energy_label(e: f64) -> &'static str {
    if e >= 0.7 {
        "精力充沛"
    } else if e >= 0.4 {
        "正常"
    } else {
        "疲惫"
    }
}

/// 将心情决策映射为多行 key=value 文本
pub fn mood_decision_label(decision: &HashMap<String, String>) -> String {
    let mut lines: Vec<String> = Vec::new();

    let participation_map: HashMap<&str, &str> = [
        ("active", "主动参与"),
        ("normal", "正常接话"),
        ("reserved", "克制参与"),
    ]
    .iter()
    .cloned()
    .collect();

    let reply_energy_map: HashMap<&str, &str> = [
        ("low", "低能量"),
        ("normal", "中等能量"),
        ("high", "积极回复"),
    ]
    .iter()
    .cloned()
    .collect();

    let risk_map: HashMap<&str, &str> =
        [("safe", "安全"), ("normal", "正常"), ("careful", "需谨慎")]
            .iter()
            .cloned()
            .collect();

    let style_map: HashMap<&str, &str> = [
        ("soft", "柔和"),
        ("playful", "轻松玩笑"),
        ("direct", "直接"),
        ("calm", "平静"),
        ("normal", "正常"),
    ]
    .iter()
    .cloned()
    .collect();

    if let Some(pb) = decision
        .get("participation_bias")
        .and_then(|v| participation_map.get(v.as_str()))
    {
        lines.push(format!("参与度={}", pb));
    }
    if let Some(re) = decision
        .get("reply_energy")
        .and_then(|v| reply_energy_map.get(v.as_str()))
    {
        lines.push(format!("回复能量={}", re));
    }
    if let Some(rp) = decision
        .get("risk_posture")
        .and_then(|v| risk_map.get(v.as_str()))
    {
        lines.push(format!("风险={}", rp));
    }
    if let Some(sb) = decision
        .get("style_bias")
        .and_then(|v| style_map.get(v.as_str()))
    {
        lines.push(format!("风格={}", sb));
    }
    if let Some(reason) = decision.get("reason") {
        if !reason.is_empty() {
            lines.push(format!("原因={}", reason));
        }
    }

    lines.join("\n")
}

/// 将对话窗口信号映射为多行 key=value 文本
pub fn conversation_window_label(window: &HashMap<String, String>) -> String {
    let mut lines: Vec<String> = Vec::new();

    let speaker_map: HashMap<&str, &str> = [
        ("assistant", "对助手说"),
        ("group", "对群聊开放"),
        ("specific_user", "对特定用户"),
        ("unknown", "不明确"),
    ]
    .iter()
    .cloned()
    .collect();

    let interrupt_map: HashMap<&str, &str> = [("low", "低"), ("medium", "中"), ("high", "高")]
        .iter()
        .cloned()
        .collect();

    let openness_map: HashMap<&str, &str> = [
        ("open", "开放"),
        ("semi_open", "半开放"),
        ("closed", "封闭"),
    ]
    .iter()
    .cloned()
    .collect();

    if let Some(ct) = window.get("current_thread") {
        if !ct.is_empty() {
            lines.push(format!("话题={}", ct));
        }
    }
    if let Some(st) = window
        .get("speaker_target")
        .and_then(|v| speaker_map.get(v.as_str()))
    {
        lines.push(format!("对象={}", st));
    }
    if let Some(ir) = window
        .get("interruption_risk")
        .and_then(|v| interrupt_map.get(v.as_str()))
    {
        lines.push(format!("插话风险={}", ir));
    }
    if let Some(co) = window
        .get("conversation_openness")
        .and_then(|v| openness_map.get(v.as_str()))
    {
        lines.push(format!("开放度={}", co));
    }

    let should_wait = window
        .get("should_wait_for_more")
        .map(|v| v == "true")
        .unwrap_or(false);
    lines.push(format!(
        "等待更多={}",
        if should_wait { "是" } else { "否" }
    ));

    if let Some(reason) = window.get("reason") {
        if !reason.is_empty() {
            lines.push(format!("原因={}", reason));
        }
    }

    lines.join("\n")
}

/// 将回复效果标签映射为用户情感倾向值
pub fn reply_effect_to_valence(effect_label: &str) -> f64 {
    let map: HashMap<&str, f64> = [("positive", 0.5), ("negative", -0.5), ("repair", -0.3)]
        .iter()
        .cloned()
        .collect();
    map.get(effect_label.trim().to_lowercase().as_str())
        .copied()
        .unwrap_or(0.0)
}

/// 构建供大模型提示词使用的可读系统状态段
///
/// 每个数值字段后跟随区间→标签映射，使大模型理解数值含义。
pub fn system_state_block(
    energy: f64,
    valence: f64,
    arousal: f64,
    gap_hours: f64,
    planner_mood_text: &str,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    if !planner_mood_text.trim().is_empty() {
        parts.push(format!(
            "【当前助手情绪】（来自上一轮分析）\n{}",
            planner_mood_text.trim()
        ));
    }

    parts.push(format!(
        "【系统状态】\n\
         energy={energy:.2}\n\
         \x20 0.80~1.00 = 充沛 | 0.60~0.80 = 较充足 | 0.40~0.60 = 正常 | 0.20~0.40 = 偏低 | 0.00~0.20 = 疲惫\n\
         valence={valence:.2}\n\
         \x20 0.30~1.00 = 积极 | 0.10~0.30 = 偏积极 | -0.10~0.10 = 中性 | -0.30~-0.10 = 略消极 | -1.00~-0.30 = 明显消极\n\
         arousal={arousal:.2}\n\
         \x20 0.60~1.00 = 兴奋 | 0.40~0.60 = 平静 | 0.00~0.40 = 低沉\n\
         gap_hours={gap_hours:.1}\n\
         \x20 0.00~0.50 = 连续对话 | 0.50~4.00 = 短间隔 | 4.00~24.00 = 长间隔 | 24.00以上 = 隔日",
        energy = energy,
        valence = valence,
        arousal = arousal,
        gap_hours = gap_hours,
    ));

    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mood_state_label_positive() {
        let label = mood_state_label(0.5, 0.8, 0.9);
        assert_eq!(label, "积极，兴奋，精力充沛");
    }

    #[test]
    fn test_mood_state_label_neutral() {
        let label = mood_state_label(0.0, 0.5, 0.5);
        assert_eq!(label, "中性，平稳，正常");
    }

    #[test]
    fn test_mood_state_label_negative() {
        let label = mood_state_label(-0.5, 0.3, 0.2);
        assert_eq!(label, "消沉，低沉，疲惫");
    }

    #[test]
    fn test_reply_effect_to_valence_positive() {
        assert!((reply_effect_to_valence("positive") - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_reply_effect_to_valence_unknown() {
        assert_eq!(reply_effect_to_valence("unknown"), 0.0);
    }

    #[test]
    fn test_system_state_block() {
        let block = system_state_block(0.75, 0.2, 0.5, 1.5, "心情不错");
        assert!(block.contains("心情不错"));
        assert!(block.contains("energy=0.75"));
        assert!(block.contains("valence=0.20"));
    }

    #[test]
    fn test_mood_decision_label() {
        let mut decision = HashMap::new();
        decision.insert("participation_bias".to_string(), "active".to_string());
        decision.insert("reply_energy".to_string(), "normal".to_string());
        decision.insert("reason".to_string(), "用户提问".to_string());
        let label = mood_decision_label(&decision);
        assert!(label.contains("主动参与"));
        assert!(label.contains("中等能量"));
        assert!(label.contains("用户提问"));
    }

    #[test]
    fn test_conversation_window_label() {
        let mut window = HashMap::new();
        window.insert("speaker_target".to_string(), "group".to_string());
        window.insert("should_wait_for_more".to_string(), "true".to_string());
        let label = conversation_window_label(&window);
        assert!(label.contains("对群聊开放"));
        assert!(label.contains("等待更多=是"));
    }
}
