use std::path::PathBuf;

use clone_scanner::Config;
use tempfile::TempDir;

use crate::{DetectConfig, DetectionResult, Kind, instances};

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
    {
        for (index, left) in group.fragments.iter().enumerate() {
            for right in group.fragments.iter().skip(index + 1) {
                let same_file = left.file == right.file;
                let byte_ranges_overlap = left.byte_range.start < right.byte_range.end
                    && right.byte_range.start < left.byte_range.end;
                let overlaps = same_file && byte_ranges_overlap;
                assert!(!overlaps, "clone group compared overlapping fragments: {group:?}");
            }
        }
    }
}
