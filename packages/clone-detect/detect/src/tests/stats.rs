use tempfile::TempDir;

use super::helpers::{create_temp_file, scan_and_run};
use crate::DetectConfig;

#[test]
fn files_scanned() {
    let dir = TempDir::new().unwrap();

    create_temp_file(&dir, "file1.rs", "fn a() {}");
    create_temp_file(&dir, "file2.rs", "fn b() {}");
    create_temp_file(&dir, "file3.rs", "fn c() {}");

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert_eq!(result.stats.files_scanned, 3);
}

#[test]
fn nodes_analyzed() {
    let dir = TempDir::new().unwrap();

    let code = r#"
fn func1() {
    println!("hello");
}

fn func2() {
    println!("world");
}
"#;
    create_temp_file(&dir, "file.rs", code);

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert!(result.stats.nodes_analyzed >= 2);
}
