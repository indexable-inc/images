use ast_merge_langs::Lang;

use super::helpers::create_temp_file;
use crate::Scanner;

#[test]
fn single_rust() {
    let dir = tempfile::tempdir().unwrap();
    let content = r"
fn foo() {
    let x = 1;
    let y = 2;
    let z = x + y;
    z
}
";
    let path = create_temp_file(dir.path(), "test.rs", content);

    let scanner = Scanner::with_defaults();
    let result = scanner.file(&path).unwrap();

    assert!(result.is_some());
    let scanned = result.unwrap();
    assert_eq!(scanned.language, Lang::Rust);
    assert!(!scanned.nodes.is_empty());
}

#[test]
fn javascript() {
    let dir = tempfile::tempdir().unwrap();
    let content = r"
function calculate(a, b) {
    const sum = a + b;
    const product = a * b;
    return sum + product;
}
";
    let path = create_temp_file(dir.path(), "test.js", content);

    let scanner = Scanner::with_defaults();
    let result = scanner.file(&path).unwrap();

    assert!(result.is_some());
    let scanned = result.unwrap();
    assert_eq!(scanned.language, Lang::JavaScript);
}

#[test]
fn python() {
    let dir = tempfile::tempdir().unwrap();
    let content = r"
def calculate(a, b):
    sum_val = a + b
    product = a * b
    return sum_val + product
";
    let path = create_temp_file(dir.path(), "test.py", content);

    let scanner = Scanner::with_defaults();
    let result = scanner.file(&path).unwrap();

    assert!(result.is_some());
    let scanned = result.unwrap();
    assert_eq!(scanned.language, Lang::Python);
}

#[test]
fn unknown_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_temp_file(dir.path(), "test.xyz", "some content");

    let scanner = Scanner::with_defaults();
    let result = scanner.file(&path).unwrap();

    assert!(result.is_none());
}

#[test]
fn empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_temp_file(dir.path(), "empty.rs", "");

    let scanner = Scanner::with_defaults();
    let result = scanner.file(&path).unwrap();

    assert!(result.is_some());
    let scanned = result.unwrap();
    assert!(scanned.nodes.is_empty());
}

#[test]
fn scanned_fields() {
    let dir = tempfile::tempdir().unwrap();
    let content = r"
fn test_function() {
    let x = 1;
    let y = 2;
    x + y
}
";
    let path = create_temp_file(dir.path(), "test.rs", content);

    let scanner = Scanner::with_defaults();
    let result = scanner.file(&path).unwrap().unwrap();

    assert_eq!(result.path, path);
    assert_eq!(result.language, Lang::Rust);
    assert!(!result.source.is_empty());
}
