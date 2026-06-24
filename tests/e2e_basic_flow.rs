mod common;

use common::test_config;

#[test]
fn test_config_has_correct_defaults() {
    let config = test_config();
    assert_eq!(config.model.primary_model, "test-model");
    assert!(config.drive.enabled);
}
