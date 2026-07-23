use ast_merge_langs::Lang;

use super::helpers::{create_temp_dir, create_temp_file};
use crate::Scanner;

#[test]
fn single_file() {
    let dir = tempfile::tempdir().unwrap();
    let content = r"
fn foo() {
    let x = 1;
    let y = 2;
    let z = x + y;
    z
}
";
    create_temp_file(dir.path(), "test.rs", content);

    let scanner = Scanner::with_defaults();
    let result = scanner.directory(dir.path()).unwrap();

    assert_eq!(result.files.len(), 1);
    assert!(!result.index.content_index.is_empty() || !result.index.normalized_index.is_empty());
}

#[test]
fn multiple_files() {
    let dir = tempfile::tempdir().unwrap();
    let content = r"
fn foo() {
    let x = 1;
    let y = 2;
    x + y
}
";
    create_temp_file(dir.path(), "file1.rs", content);
    create_temp_file(dir.path(), "file2.rs", content);
    create_temp_file(dir.path(), "file3.rs", content);

    let scanner = Scanner::with_defaults();
    let result = scanner.directory(dir.path()).unwrap();

    assert_eq!(result.files.len(), 3);
}

#[test]
fn nested_directories() {
    let dir = tempfile::tempdir().unwrap();
    let src_dir = create_temp_dir(dir.path(), "src");
    let lib_dir = create_temp_dir(&src_dir, "lib");

    let content = r"
fn nested() {
    let a = 1;
    let b = 2;
    a + b
}
";
    create_temp_file(dir.path(), "root.rs", content);
    create_temp_file(&src_dir, "src.rs", content);
    create_temp_file(&lib_dir, "lib.rs", content);

    let scanner = Scanner::with_defaults();
    let result = scanner.directory(dir.path()).unwrap();

    assert_eq!(result.files.len(), 3);
}

#[test]
fn ignores_node_modules() {
    let dir = tempfile::tempdir().unwrap();

    create_temp_dir(dir.path(), ".git");
    create_temp_file(dir.path(), ".gitignore", "node_modules/\n");

    let node_modules = create_temp_dir(dir.path(), "node_modules");

    let content = r"
fn foo() {
    let x = 1;
    let y = 2;
    x + y
}
";
    create_temp_file(dir.path(), "main.rs", content);
    create_temp_file(&node_modules, "ignored.js", "function foo() { return 1; }");

    let scanner = Scanner::with_defaults();
    let result = scanner.directory(dir.path()).unwrap();

    assert_eq!(result.files.len(), 1);
    assert!(
        result
            .files
            .first()
            .unwrap()
            .path
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("main.rs"))
    );
}

#[test]
fn ignores_target() {
    let dir = tempfile::tempdir().unwrap();

    create_temp_dir(dir.path(), ".git");
    create_temp_file(dir.path(), ".gitignore", "target/\n");

    let target_dir = create_temp_dir(dir.path(), "target");

    let content = r"
fn foo() {
    let x = 1;
    let y = 2;
    x + y
}
";
    create_temp_file(dir.path(), "main.rs", content);
    create_temp_file(&target_dir, "ignored.rs", content);

    let scanner = Scanner::with_defaults();
    let result = scanner.directory(dir.path()).unwrap();

    assert_eq!(result.files.len(), 1);
}

#[test]
fn mixed_languages() {
    let dir = tempfile::tempdir().unwrap();

    let rust_content = r"
fn rust_func() {
    let x = 1;
    let y = 2;
    x + y
}
";
    let js_content = r"
function jsFunc() {
    const x = 1;
    const y = 2;
    return x + y;
}
";
    let py_content = r"
def py_func():
    x = 1
    y = 2
    return x + y
";

    create_temp_file(dir.path(), "main.rs", rust_content);
    create_temp_file(dir.path(), "app.js", js_content);
    create_temp_file(dir.path(), "script.py", py_content);

    let scanner = Scanner::with_defaults();
    let result = scanner.directory(dir.path()).unwrap();

    assert_eq!(result.files.len(), 3);

    let languages: Vec<_> = result.files.iter().map(|f| f.language).collect();
    assert!(languages.contains(&Lang::Rust));
    assert!(languages.contains(&Lang::JavaScript));
    assert!(languages.contains(&Lang::Python));
}
