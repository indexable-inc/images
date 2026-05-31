use std::{io::Write as _, path::PathBuf};

use clone_detect::{DetectConfig, DetectionResult, instances};
use clone_scanner::Config;
use tempfile::TempDir;

pub const fn test_scan_config() -> Config {
    Config {
        min_lines: 1,
        min_nodes: 1,
        respect_gitignore: false,
        include_hidden: false,
    }
}

pub fn create_temp_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
    let path = dir.path().join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
    path
}

/// Scan a directory with the default test config and detect instances with the
/// given [`DetectConfig`].
pub fn scan_and_detect(dir: &TempDir, detect_config: &DetectConfig) -> DetectionResult {
    let scanner = clone_scanner::Scanner::new(test_scan_config());
    let scan = scanner.directory(dir.path()).unwrap();
    instances(&scan, detect_config)
}
