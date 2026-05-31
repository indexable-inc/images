use tempfile::TempDir;

use super::helpers::{create_temp_file, scan_and_run};
use crate::{DetectConfig, Kind};

#[test]
fn type1_exact_duplicates() {
    let dir = TempDir::new().unwrap();

    let code = r"
fn calculate_sum(a: i32, b: i32) -> i32 {
    let result = a + b;
    result
}
";
    create_temp_file(&dir, "file1.rs", code);
    create_temp_file(&dir, "file2.rs", code);

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert!(
        result.stats.type1_groups > 0,
        "Should detect Type-1 instances"
    );
    assert!(
        result
            .instances
            .iter()
            .any(|c| matches!(c.clone_type, Kind::Type1)),
        "Should have Type-1 clone groups"
    );
}

#[test]
fn type1_same_file_duplicates() {
    let dir = TempDir::new().unwrap();

    let code = r#"
fn calculate(a: i32, b: i32) -> i32 {
    let result = a + b;
    result
}

fn other_stuff() {
    println!("hello");
}

fn calculate_again(a: i32, b: i32) -> i32 {
    let result = a + b;
    result
}
"#;
    create_temp_file(&dir, "file.rs", code);

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert!(result.stats.type1_groups > 0 || result.stats.type2_groups > 0);
}

#[test]
fn type2_renamed_identifiers() {
    let dir = TempDir::new().unwrap();

    let code1 = r"
fn calculate_sum(a: i32, b: i32) -> i32 {
    let result = a + b;
    result
}
";
    let code2 = r"
fn compute_total(x: i32, y: i32) -> i32 {
    let output = x + y;
    output
}
";
    create_temp_file(&dir, "file1.rs", code1);
    create_temp_file(&dir, "file2.rs", code2);

    let result = scan_and_run(&dir, &DetectConfig::default());

    let has_clones = result.stats.type1_groups > 0 || result.stats.type2_groups > 0;
    assert!(
        has_clones,
        "Should detect Type-2 instances with renamed identifiers"
    );
}

#[test]
fn no_clones_unique_files() {
    let dir = TempDir::new().unwrap();

    let code1 = r#"
fn function_one() {
    println!("This is function one");
}
"#;
    let code2 = r#"
fn function_two() {
    let x = 42;
    let y = x * 2;
    println!("Result: {}", y);
}
"#;
    let code3 = r"
struct MyStruct {
    field: i32,
}
impl MyStruct {
    fn new(value: i32) -> Self {
        Self { field: value }
    }
}
";
    create_temp_file(&dir, "file1.rs", code1);
    create_temp_file(&dir, "file2.rs", code2);
    create_temp_file(&dir, "file3.rs", code3);

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert_eq!(result.stats.type1_groups, 0);
}

#[test]
fn empty_scan() {
    let scan = clone_scanner::Output {
        files: vec![],
        index: clone_scanner::Hash::new(),
    };

    let result = crate::instances(&scan, &DetectConfig::default());

    assert!(result.instances.is_empty());
    assert_eq!(result.stats.files_scanned, 0);
    assert_eq!(result.stats.nodes_analyzed, 0);
    assert_eq!(result.stats.type1_groups, 0);
    assert_eq!(result.stats.type2_groups, 0);
    assert_eq!(result.stats.type3_groups, 0);
}

#[test]
fn single_file_no_clones() {
    let dir = TempDir::new().unwrap();

    let code = r#"
fn unique_function() {
    println!("hello");
}
"#;
    create_temp_file(&dir, "file.rs", code);

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert_eq!(result.stats.type1_groups, 0);
    assert_eq!(result.stats.files_scanned, 1);
}

#[test]
fn group_has_multiple_fragments() {
    let dir = TempDir::new().unwrap();

    let code = "fn dup() { println!(\"hello\"); }";
    create_temp_file(&dir, "file1.rs", code);
    create_temp_file(&dir, "file2.rs", code);
    create_temp_file(&dir, "file3.rs", code);

    let result = scan_and_run(&dir, &DetectConfig::default());

    let has_multi_fragment = result.instances.iter().any(|g| g.fragments.len() >= 2);
    assert!(
        has_multi_fragment,
        "Clone groups should have multiple fragments"
    );
}

#[test]
fn fragment_contains_correct_info() {
    let dir = TempDir::new().unwrap();

    let code = r#"fn duplicate() {
    println!("hello");
}
"#;
    create_temp_file(&dir, "file1.rs", code);
    create_temp_file(&dir, "file2.rs", code);

    let result = scan_and_run(&dir, &DetectConfig::default());

    if let Some(clone_group) = result.instances.first() {
        for fragment in &clone_group.fragments {
            assert!(fragment.byte_range.start < fragment.byte_range.end);
            assert!(fragment.lines.start <= fragment.lines.end);
            assert!(!fragment.kind.is_empty());
            assert!(fragment.file.exists());
        }
    }
}
