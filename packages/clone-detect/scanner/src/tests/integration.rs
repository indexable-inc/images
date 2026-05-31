use super::helpers::{create_temp_file, test_scan_config};
use crate::{Config, Scanner};

#[test]
fn config_defaults() {
    let config = Config::default();
    assert_eq!(config.min_lines, 5);
    assert_eq!(config.min_nodes, 10);
    assert!(config.respect_gitignore);
    assert!(!config.include_hidden);
}

#[test]
fn config_custom() {
    let config = Config {
        min_lines: 10,
        min_nodes: 20,
        respect_gitignore: false,
        include_hidden: true,
    };

    assert_eq!(config.min_lines, 10);
    assert_eq!(config.min_nodes, 20);
    assert!(!config.respect_gitignore);
    assert!(config.include_hidden);
}

#[test]
fn total_nodes_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let content = r"
fn foo() {
    let x = 1;
    let y = 2;
    x + y
}

fn bar() {
    let a = 3;
    let b = 4;
    a * b
}
";
    create_temp_file(dir.path(), "test.rs", content);

    let scanner = Scanner::with_defaults();
    let result = scanner.directory(dir.path()).unwrap();

    assert!(result.total_nodes() > 0);
    assert_eq!(result.total_files(), 1);
}

#[test]
fn detects_duplicate_functions() {
    let dir = tempfile::tempdir().unwrap();

    let content = r"
fn duplicate_function() {
    let x = 1;
    let y = 2;
    let z = x + y;
    z
}
";
    create_temp_file(dir.path(), "file1.rs", content);
    create_temp_file(dir.path(), "file2.rs", content);

    let scanner = Scanner::new(test_scan_config());
    let result = scanner.directory(dir.path()).unwrap();

    assert_eq!(result.files.len(), 2);
    assert!(result.index.type1_candidates().next().is_some());
}

#[test]
fn detects_renamed_functions() {
    let dir = tempfile::tempdir().unwrap();

    let content1 = r"
fn add(a: i32, b: i32) -> i32 {
    let sum = a + b;
    sum
}
";
    let content2 = r"
fn sum(x: i32, y: i32) -> i32 {
    let result = x + y;
    result
}
";
    create_temp_file(dir.path(), "file1.rs", content1);
    create_temp_file(dir.path(), "file2.rs", content2);

    let scanner = Scanner::new(test_scan_config());
    let result = scanner.directory(dir.path()).unwrap();

    assert_eq!(result.files.len(), 2);
    assert!(result.index.type2_candidates().next().is_some());
}
