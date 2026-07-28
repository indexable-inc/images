use crate::DetectConfig;

#[test]
fn default_cfg() {
    let config = DetectConfig::default();
    assert!(!config.enable_type3);
    assert!((config.type3_threshold - 0.7).abs() < 0.001);
    assert!(!config.enable_sequences);
}

#[test]
fn custom_cfg() {
    let config = DetectConfig {
        enable_type3: true,
        type3_threshold: 0.8,
        ..DetectConfig::default()
    };
    assert!(config.enable_type3);
    assert!((config.type3_threshold - 0.8).abs() < 0.001);
}
