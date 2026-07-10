use tempfile::TempDir;

use super::helpers::{assert_no_overlapping_fragments, create_temp_file, scan_and_run};
use crate::DetectConfig;

#[test]
fn full_rust() {
    let dir = TempDir::new().unwrap();

    let utils_code = r"
pub fn validate_input(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    input.chars().all(|c| c.is_alphanumeric())
}

pub fn sanitize_string(s: &str) -> String {
    s.trim().to_lowercase()
}
";

    let handlers_code = r#"
pub fn validate_input(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    input.chars().all(|c| c.is_alphanumeric())
}

pub fn handle_request(data: &str) -> String {
    format!("Handled: {}", data)
}
"#;

    create_temp_file(&dir, "src/utils.rs", utils_code);
    create_temp_file(&dir, "src/handlers.rs", handlers_code);

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert_eq!(result.stats.files_scanned, 2);
    assert!(
        result.stats.type1_groups > 0,
        "Should detect exact duplicate function"
    );
}

#[test]
fn full_with_type3() {
    let dir = TempDir::new().unwrap();

    let code1 = r"
fn process_items(items: Vec<i32>) -> i32 {
    let mut sum = 0;
    for item in items {
        sum += item;
    }
    sum
}
";

    let code2 = r"
fn aggregate_values(values: Vec<i32>) -> i32 {
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
        type3_threshold: 0.7,
        ..DetectConfig::default()
    };
    let result = scan_and_run(&dir, &config);

    assert_eq!(result.stats.files_scanned, 2);
}

#[test]
fn sequences_never_compare_overlapping_regions_of_one_file() {
    let dir = TempDir::new().unwrap();
    let code = r#"
fn process(values: &[i32]) -> i32 {
    let mut total = 0;
    for value in values {
        total += value;
    }
    println!("{total}");
    total
}
"#;
    create_temp_file(&dir, "nested.rs", code);

    let result = scan_and_run(
        &dir,
        &DetectConfig {
            enable_sequences: true,
            sequence_window_size: 2,
            ..DetectConfig::default()
        },
    );

    assert_no_overlapping_fragments(&result, |kind| {
        matches!(kind, crate::Kind::Sequence { .. })
    });
}

#[test]
fn comments_do_not_form_statement_sequences() {
    let dir = TempDir::new().unwrap();
    create_temp_file(
        &dir,
        "first.rs",
        "fn first() {\n// one\n// two\n// three\nlet left = 1;\n}\n",
    );
    create_temp_file(
        &dir,
        "second.rs",
        "fn second() {\n// alpha\n// beta\n// gamma\nreturn;\n}\n",
    );

    let result = scan_and_run(
        &dir,
        &DetectConfig {
            enable_sequences: true,
            sequence_window_size: 2,
            ..DetectConfig::default()
        },
    );

    assert!(
        result.instances.iter().filter(|group| matches!(group.clone_type, crate::Kind::Sequence { .. })).all(|group| {
            group.fragments.iter().all(|fragment| fragment.lines.end - fragment.lines.start < 3)
        }),
        "comments inflated a sequence fragment: {:#?}",
        result.instances,
    );
}
