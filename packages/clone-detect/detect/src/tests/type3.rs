use tempfile::TempDir;

use super::helpers::{create_temp_file, scan_and_run};
use crate::DetectConfig;

#[test]
fn disabled_by_default() {
    let dir = TempDir::new().unwrap();

    let code1 = r"
fn process_data(items: Vec<i32>) -> i32 {
    let mut sum = 0;
    for item in items {
        sum += item;
    }
    sum
}
";
    let code2 = r#"
fn process_numbers(values: Vec<i32>) -> i32 {
    let mut total = 0;
    for value in values {
        total += value;
        println!("Added {}", value);
    }
    total
}
"#;
    create_temp_file(&dir, "file1.rs", code1);
    create_temp_file(&dir, "file2.rs", code2);

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert_eq!(
        result.stats.type3_groups, 0,
        "Type-3 should be disabled by default"
    );
}

#[test]
fn enabled() {
    let dir = TempDir::new().unwrap();

    let code1 = r"
fn process(items: Vec<i32>) -> i32 {
    let mut sum = 0;
    for item in items {
        sum += item;
    }
    sum
}
";
    let code2 = r"
fn handle(values: Vec<i32>) -> i32 {
    let mut total = 0;
    for value in values {
        total += value;
    }
    total
}
";
    create_temp_file(&dir, "file1.rs", code1);
    create_temp_file(&dir, "file2.rs", code2);

    let config = DetectConfig {
        enable_type3: true,
        type3_threshold: 0.5,
        ..DetectConfig::default()
    };
    let result = scan_and_run(&dir, &config);

    let _ = result.stats.type3_groups;
}

#[test]
fn high_threshold() {
    let dir = TempDir::new().unwrap();

    let code1 = r#"
fn short_func() {
    println!("hello");
}
"#;
    let code2 = r#"
fn long_func() {
    println!("hello");
    println!("world");
    println!("more");
    println!("stuff");
    println!("here");
}
"#;
    create_temp_file(&dir, "file1.rs", code1);
    create_temp_file(&dir, "file2.rs", code2);

    let config = DetectConfig {
        enable_type3: true,
        type3_threshold: 0.95,
        ..DetectConfig::default()
    };
    let result = scan_and_run(&dir, &config);

    let _ = result.stats.type3_groups;
}
