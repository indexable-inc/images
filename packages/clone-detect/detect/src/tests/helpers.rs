use std::path::PathBuf;

use clone_scanner::Config;
use tempfile::TempDir;

use crate::{DetectConfig, DetectionResult, instances};

pub fn create_temp_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
    clone_test_support::write_file(dir.path(), name, content)
}

pub fn test_scan_config() -> Config {
    Config::for_tests()
}

/// Scan a directory with the default test config and detect instances with the
/// given [`DetectConfig`].
pub fn scan_and_run(dir: &TempDir, detect_config: &DetectConfig) -> DetectionResult {
    let scanner = clone_scanner::Scanner::new(test_scan_config());
    let scan = scanner.directory(dir.path()).unwrap();
    instances(&scan, detect_config)
}
