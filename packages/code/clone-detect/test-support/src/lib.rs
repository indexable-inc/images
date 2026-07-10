//! Shared filesystem fixture helpers for the clone detector crates.

use std::path::{Path, PathBuf};

/// Write a fixture below `root`, creating any nested parent directories.
///
/// # Panics
///
/// Panics when the fixture directory or file cannot be written.
#[must_use]
pub fn write_file(root: &Path, relative: &str, content: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create fixture parent directory");
    }
    std::fs::write(&path, content).expect("write fixture file");
    path
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
