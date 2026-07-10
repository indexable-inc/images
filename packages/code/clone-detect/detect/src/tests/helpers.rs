use std::{io::Write as _, path::PathBuf};

use clone_scanner::Config;
use tempfile::TempDir;

use crate::{DetectConfig, DetectionResult, Kind, instances};

pub fn create_temp_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
    let path = dir.path().join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
    path
}

pub fn test_scan_config() -> Config {
    Config {
        min_lines: 1,
        min_nodes: 1,
        respect_gitignore: false,
        include_hidden: false,
    }
}

/// Scan a directory with the default test config and detect instances with the
/// given [`DetectConfig`].
pub fn scan_and_run(dir: &TempDir, detect_config: &DetectConfig) -> DetectionResult {
    let scanner = clone_scanner::Scanner::new(test_scan_config());
    let scan = scanner.directory(dir.path()).unwrap();
    instances(&scan, detect_config)
}

pub fn assert_no_overlapping_fragments(
    result: &DetectionResult,
    selected: impl Fn(&Kind) -> bool,
) {
    for group in result
        .instances
        .iter()
        .filter(|group| selected(&group.clone_type))
    {
        for (index, left) in group.fragments.iter().enumerate() {
            for right in group.fragments.iter().skip(index + 1) {
                let overlaps = left.file == right.file
                    && left.byte_range.start < right.byte_range.end
                    && right.byte_range.start < left.byte_range.end;
                assert!(!overlaps, "clone group compared overlapping fragments: {group:?}");
            }
        }
    }
}
