use std::path::{Path, PathBuf};

use crate::Config;

pub fn create_temp_file(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path).unwrap();
    std::io::Write::write_all(&mut file, content.as_bytes()).unwrap();
    path
}

pub fn create_temp_dir(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    std::fs::create_dir_all(&path).unwrap();
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
