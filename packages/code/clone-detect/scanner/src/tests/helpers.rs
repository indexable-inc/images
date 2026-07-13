use std::path::{Path, PathBuf};

use crate::Config;

pub fn create_temp_file(dir: &Path, name: &str, content: &str) -> PathBuf {
    clone_test_support::write_file(dir, name, content)
}

pub fn create_temp_dir(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}

pub fn test_scan_config() -> Config {
    Config::for_tests()
}
