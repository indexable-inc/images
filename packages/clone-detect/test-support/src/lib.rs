//! Shared filesystem fixture helpers for the clone detector crates.

use std::path::{Path, PathBuf};

/// Write a fixture below `root`, creating any nested parent directories.
///
/// # Panics
///
/// Panics if the fixture directory or file cannot be written.
#[must_use]
pub fn write_file(root: &Path, relative: &str, content: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture parent directory");
    }
    std::fs::write(&path, content).expect("write fixture file");
    path
}

/// Scan a single-file fixture and assert the detector never pairs two
/// overlapping byte ranges of that one file within a selected clone kind.
///
/// The fixture `code` is written to one file, scanned with the test scanner
/// config, and analyzed with `config`. `selected` picks which clone kinds the
/// invariant applies to (e.g. Type-3 groups or statement sequences).
///
/// # Panics
///
/// Panics if the scan fails, or if a selected clone group compares two
/// fragments from the same file whose byte ranges overlap.
#[cfg(feature = "detect")]
pub fn assert_single_file_has_no_overlapping_fragments(
    code: &str,
    config: &clone_detect::DetectConfig,
    selected: impl Fn(&clone_detect::Kind) -> bool,
) {
    let dir = tempfile::TempDir::new().expect("create temp dir");
    let _ = write_file(dir.path(), "nested.rs", code);

    let scanner = clone_scanner::Scanner::new(clone_scanner::Config::for_tests());
    let scan = scanner
        .directory(dir.path())
        .expect("scan fixture directory");
    let result = clone_detect::instances(&scan, config);

    for group in result
        .instances
        .iter()
        .filter(|group| selected(&group.clone_type))
    {
        for (index, left) in group.fragments.iter().enumerate() {
            for right in group.fragments.iter().skip(index + 1) {
                let same_file = left.file == right.file;
                let left_starts_before_right_ends = left.byte_range.start < right.byte_range.end;
                let right_starts_before_left_ends = right.byte_range.start < left.byte_range.end;
                let overlaps =
                    same_file && left_starts_before_right_ends && right_starts_before_left_ends;
                assert!(
                    !overlaps,
                    "clone group compared overlapping fragments: {group:?}"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::write_file;

    #[test]
    fn writes_nested_fixture() {
        let root = std::env::temp_dir().join(format!("clone-test-support-{}", std::process::id()));
        let path = write_file(&root, "nested/example.rs", "fn example() {}\n");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn example() {}\n");
        std::fs::remove_dir_all(root).unwrap();
    }
}
