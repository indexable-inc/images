#![expect(clippy::unwrap_used, reason = "Test code")]

mod common;

use clone_detect::{DetectConfig, DetectionResult, Kind};
use clone_scanner::Config;
use common::{create_temp_file, scan_and_detect};
use tempfile::TempDir;

#[test]
fn full_pipeline_simple_rust_project() {
    let dir = TempDir::new().unwrap();

    create_temp_file(
        &dir,
        "src/lib.rs",
        r#"
pub mod utils;
pub mod handlers;

pub fn main_entry() {
    println!("Main entry point");
}
"#,
    );

    create_temp_file(
        &dir,
        "src/utils.rs",
        r"
pub fn calculate_hash(data: &[u8]) -> u64 {
    let mut hash = 0u64;
    for byte in data {
        hash = hash.wrapping_mul(31).wrapping_add(*byte as u64);
    }
    hash
}

pub fn validate_input(input: &str) -> bool {
    !input.is_empty() && input.len() < 1000
}
",
    );

    create_temp_file(
        &dir,
        "src/handlers.rs",
        r#"
pub fn calculate_hash(data: &[u8]) -> u64 {
    let mut hash = 0u64;
    for byte in data {
        hash = hash.wrapping_mul(31).wrapping_add(*byte as u64);
    }
    hash
}

pub fn process_request(data: &str) -> String {
    format!("Processed: {}", data)
}
"#,
    );

    let result = scan_and_detect(&dir, &DetectConfig::default());

    assert_eq!(result.stats.files_scanned, 3);
    assert!(
        result.stats.type1_groups > 0,
        "Should detect duplicate calculate_hash function"
    );

    assert!(!result.instances.is_empty());

    let type1_clones: Vec<_> = result
        .instances
        .iter()
        .filter(|c| matches!(c.clone_type, Kind::Type1))
        .collect();
    assert!(!type1_clones.is_empty());

    for clone in type1_clones {
        assert!(clone.fragments.len() >= 2);
        for fragment in &clone.fragments {
            assert!(fragment.file.exists());
        }
    }
}

#[test]
fn full_pipeline_multi_language_project() {
    let dir = TempDir::new().unwrap();

    create_temp_file(
        &dir,
        "src/main.rs",
        r"
fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}

fn multiply_numbers(a: i32, b: i32) -> i32 {
    a * b
}
",
    );

    create_temp_file(
        &dir,
        "src/index.ts",
        r"
function addNumbers(a: number, b: number): number {
    return a + b;
}

function multiplyNumbers(a: number, b: number): number {
    return a * b;
}
",
    );

    create_temp_file(
        &dir,
        "src/main.py",
        r"
def add_numbers(a, b):
    return a + b

def multiply_numbers(a, b):
    return a * b
",
    );

    create_temp_file(
        &dir,
        "src/utils.rs",
        r"
fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}
",
    );

    let result = scan_and_detect(&dir, &DetectConfig::default());

    assert_eq!(result.stats.files_scanned, 4);

    assert!(
        result.stats.type1_groups > 0 || result.stats.type2_groups > 0,
        "Should detect instances within language"
    );
}

#[test]
fn full_pipeline_with_subdirectories() {
    let dir = TempDir::new().unwrap();

    let code = r#"
fn duplicate_function() {
    let x = 1;
    let y = 2;
    let z = x + y;
    println!("{}", z);
}
"#;

    create_temp_file(&dir, "src/module_a/utils.rs", code);
    create_temp_file(&dir, "src/module_b/helpers.rs", code);
    create_temp_file(&dir, "src/module_c/sub/deep/file.rs", code);

    let result = scan_and_detect(&dir, &DetectConfig::default());

    assert_eq!(result.stats.files_scanned, 3);
    assert!(
        result.stats.type1_groups > 0,
        "Should detect instances across nested directories"
    );

    let has_three_way_clone = result.instances.iter().any(|c| c.fragments.len() == 3);
    assert!(has_three_way_clone, "Should find three-way clone");
}

#[test]
fn full_pipeline_type3_detection() {
    let dir = TempDir::new().unwrap();

    create_temp_file(
        &dir,
        "file1.rs",
        r"
fn process_items(items: Vec<i32>) -> i32 {
    let mut sum = 0;
    for item in items {
        sum += item;
    }
    sum
}
",
    );

    create_temp_file(
        &dir,
        "file2.rs",
        r"
fn process_values(values: Vec<i32>) -> i32 {
    let mut total = 0;
    for val in values {
        total += val;
    }
    total
}
",
    );

    let scanner = clone_scanner::Scanner::new(common::test_scan_config());
    let scan = scanner.directory(dir.path()).unwrap();

    let result_no_type3 = clone_detect::instances(&scan, &DetectConfig::default());
    assert_eq!(result_no_type3.stats.type3_groups, 0);

    let config = DetectConfig {
        enable_type3: true,
        type3_threshold: 0.5,
        ..DetectConfig::default()
    };
    let result_with_type3 = clone_detect::instances(&scan, &config);

    assert!(
        result_with_type3.stats.files_scanned == 2,
        "Should scan both files"
    );
}

#[test]
fn full_pipeline_no_false_positives() {
    let dir = TempDir::new().unwrap();

    create_temp_file(
        &dir,
        "math.rs",
        r"
pub fn factorial(n: u64) -> u64 {
    if n <= 1 {
        1
    } else {
        n * factorial(n - 1)
    }
}
",
    );

    create_temp_file(
        &dir,
        "strings.rs",
        r#"
pub fn reverse_string(s: &str) -> String {
    s.chars().rev().collect()
}

pub fn count_vowels(s: &str) -> usize {
    s.chars().filter(|c| "aeiou".contains(*c)).count()
}
"#,
    );

    create_temp_file(
        &dir,
        "collections.rs",
        r"
pub fn find_max(nums: &[i32]) -> Option<i32> {
    nums.iter().copied().max()
}

pub fn find_min(nums: &[i32]) -> Option<i32> {
    nums.iter().copied().min()
}
",
    );

    let result = scan_and_detect(&dir, &DetectConfig::default());

    assert_eq!(result.stats.files_scanned, 3);

    assert_eq!(
        result.stats.type1_groups, 0,
        "Should not have false positive Type-1 instances"
    );
}

#[test]
fn full_pipeline_large_project() {
    let dir = TempDir::new().unwrap();

    for i in 0..10 {
        create_temp_file(
            &dir,
            &format!("src/module_{i}/lib.rs"),
            &format!(
                r#"
pub fn unique_function_{i}() {{
    println!("Function {i}", {i});
}}
"#
            ),
        );
    }

    let duplicate_code = r"
fn shared_helper() {
    let config = load_config();
    process_config(config);
}
";
    create_temp_file(&dir, "src/module_0/helpers.rs", duplicate_code);
    create_temp_file(&dir, "src/module_5/helpers.rs", duplicate_code);
    create_temp_file(&dir, "src/module_9/helpers.rs", duplicate_code);

    let result = scan_and_detect(&dir, &DetectConfig::default());

    assert_eq!(result.stats.files_scanned, 13);
    assert!(
        result.stats.type1_groups > 0,
        "Should detect the 3 duplicate helper files"
    );
}

#[test]
fn full_pipeline_json_output() {
    let dir = TempDir::new().unwrap();

    let code = r#"
fn json_test() {
    let x = 42;
    println!("{}", x);
}
"#;
    create_temp_file(&dir, "file1.rs", code);
    create_temp_file(&dir, "file2.rs", code);

    let result = scan_and_detect(&dir, &DetectConfig::default());

    let json = serde_json::to_string_pretty(&result).unwrap();
    assert!(json.contains("instances"));
    assert!(json.contains("stats"));
    assert!(json.contains("files_scanned"));

    let deserialized: DetectionResult = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.stats.files_scanned, result.stats.files_scanned);
    assert_eq!(deserialized.instances.len(), result.instances.len());
}

#[test]
fn full_pipeline_with_gitignore() {
    let dir = TempDir::new().unwrap();

    let duplicate = r#"fn dup() { println!("hello"); }"#;

    std::fs::create_dir_all(dir.path().join(".git")).unwrap();

    create_temp_file(&dir, ".gitignore", "target/\nnode_modules/\n");

    create_temp_file(&dir, "src/main.rs", duplicate);
    create_temp_file(&dir, "src/lib.rs", duplicate);

    create_temp_file(&dir, "target/debug/generated.rs", duplicate);
    create_temp_file(&dir, "node_modules/dep/src/main.rs", duplicate);

    let config = Config {
        min_lines: 1,
        min_nodes: 1,
        respect_gitignore: true,
        include_hidden: false,
    };

    let scanner = clone_scanner::Scanner::new(config);
    let scan = scanner.directory(dir.path()).unwrap();
    let result = clone_detect::instances(&scan, &DetectConfig::default());

    assert_eq!(result.stats.files_scanned, 2);
}

#[test]
fn regression_fragments_have_valid_paths() {
    let dir = TempDir::new().unwrap();

    let code = "fn test() { }";
    create_temp_file(&dir, "a.rs", code);
    create_temp_file(&dir, "b.rs", code);

    let result = scan_and_detect(&dir, &DetectConfig::default());

    for clone in &result.instances {
        for fragment in &clone.fragments {
            assert!(
                fragment.file.exists(),
                "Fragment path should exist: {:?}",
                fragment.file
            );
            assert!(
                fragment.file.is_absolute() || fragment.file.starts_with(dir.path()),
                "Fragment path should be valid: {:?}",
                fragment.file
            );
        }
    }
}
