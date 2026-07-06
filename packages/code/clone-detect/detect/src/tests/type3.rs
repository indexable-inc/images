use tempfile::TempDir;

use super::helpers::{create_temp_file, scan_and_run};
use crate::{DetectConfig, DetectionResult, Kind, Type3Metric};

/// The original function for the moderately-Type-3 fixture.
const MT3_ORIGINAL: &str = r"
fn accumulate(values: &[i32]) -> i32 {
    let mut total = 0;
    for value in values {
        if *value > 0 {
            total += value;
        }
    }
    total
}
";

/// [`MT3_ORIGINAL`] copied and then edited: a running count, a running max, and
/// a final log line inserted. Same skeleton, several inserted statements — the
/// dominant real-world "copy-paste then edit" clone (moderately Type-3 in
/// `BigCloneBench` terms). The inserts grow the feature multiset enough that
/// symmetric Jaccard falls below 0.7 while the original stays nearly fully
/// contained (overlap ~1.0).
const MT3_EDITED: &str = r#"
fn accumulate(values: &[i32]) -> i32 {
    let mut total = 0;
    let mut count = 0;
    let mut maximum = i32::MIN;
    for value in values {
        if *value > 0 {
            total += value;
        }
        count += 1;
        if *value > maximum {
            maximum = *value;
        }
    }
    println!("count={} max={}", count, maximum);
    total
}
"#;

/// Does any Type-3 group pair up two whole `function_item` fragments?
fn has_function_pair_group(result: &DetectionResult) -> bool {
    result.instances.iter().any(|g| {
        matches!(g.clone_type, Kind::Type3 { .. })
            && g.fragments.len() >= 2
            && g.fragments.iter().all(|f| f.kind == "function_item")
    })
}

fn run_mt3(metric: Type3Metric) -> DetectionResult {
    let dir = TempDir::new().unwrap();
    create_temp_file(&dir, "orig.rs", MT3_ORIGINAL);
    create_temp_file(&dir, "edited.rs", MT3_EDITED);
    let config = DetectConfig {
        enable_type3: true,
        type3_threshold: 0.7,
        type3_metric: metric,
        ..DetectConfig::default()
    };
    scan_and_run(&dir, &config)
}

/// The asymmetry that motivates the overlap metric: at the default 0.7
/// threshold, the copy-with-inserted-statements pair is caught by `overlap` and
/// missed by `jaccard`. If this stops holding, either the fixture drifted or a
/// metric changed semantics.
#[test]
fn overlap_catches_inserted_statement_clone_jaccard_misses() {
    let jaccard = run_mt3(Type3Metric::Jaccard);
    assert!(
        !has_function_pair_group(&jaccard),
        "jaccard at 0.7 should miss the insert-heavy clone (that is its documented weakness)"
    );

    let overlap = run_mt3(Type3Metric::Overlap);
    assert!(
        has_function_pair_group(&overlap),
        "overlap at 0.7 must catch the insert-heavy clone"
    );
}

/// Every reported Type-3 group must be labeled with the metric that produced
/// its similarity score, so downstream consumers can interpret the number.
#[test]
fn type3_groups_carry_their_metric() {
    let result = run_mt3(Type3Metric::Overlap);
    for group in &result.instances {
        if let Kind::Type3 { metric, .. } = group.clone_type {
            assert_eq!(metric, Type3Metric::Overlap);
        }
    }
    assert!(
        result
            .instances
            .iter()
            .any(|g| matches!(g.clone_type, Kind::Type3 { .. })),
        "fixture must produce at least one Type-3 group under overlap"
    );
}

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
