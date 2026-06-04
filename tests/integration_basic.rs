/// xueli-core integration tests
use xueli_core::core::config::XueliConfig;

#[test]
fn test_default_config() {
    let config = XueliConfig::default();
    assert_eq!(config.model.primary_model, "gpt-4o");
    assert_eq!(config.model.light_model, "gpt-4o-mini");
    assert!(config.emoji.enabled);
    assert!(!config.proactive_share.enabled);
}

#[test]
fn test_config_serialization() {
    let config = XueliConfig::default();
    let toml_str = toml::to_string_pretty(&config).expect("序列化失败");
    let parsed: XueliConfig = toml::from_str(&toml_str).expect("反序列化失败");
    assert_eq!(parsed.model.primary_model, config.model.primary_model);
}
