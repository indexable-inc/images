use tempfile::TempDir;

use super::helpers::{create_temp_file, scan_and_run};
use crate::DetectConfig;

/// Two functions with identical structure but different variable names and one
/// extra statement should be detected as Type 3 instances with high similarity
/// (structure mostly the same, just one statement added).
#[test]
fn type3_one_extra_statement() {
    let dir = TempDir::new().unwrap();

    create_temp_file(
        &dir,
        "file1.rs",
        r"
fn process(items: Vec<i32>) -> Vec<i32> {
    let mut result = Vec::new();
    for item in items {
        if item > 0 {
            result.push(item);
        }
    }
    result
}
",
    );

    create_temp_file(
        &dir,
        "file2.rs",
        r"
fn filter(values: Vec<i32>) -> Vec<i32> {
    let mut output = Vec::new();
    for val in values {
        if val > 0 {
            output.push(val);
        }
    }
    output.sort();
    output
}
",
    );

    let config = DetectConfig {
        enable_type3: true,
        type3_threshold: 0.6,
        ..DetectConfig::default()
    };
    let result = scan_and_run(&dir, &config);

    assert!(
        result.stats.type3_groups > 0 || result.stats.type2_groups > 0,
        "Should detect the structurally similar functions as instances"
    );
}

/// Two completely different functions of similar size should NOT be detected
/// as Type 3 instances. The old node-count-ratio metric would match these because
/// they have similar node counts.
#[test]
fn type3_no_false_positive_same_size_different_structure() {
    let dir = TempDir::new().unwrap();

    create_temp_file(
        &dir,
        "file1.rs",
        r#"
fn render_html(items: Vec<String>) -> String {
    let mut html = String::from("<ul>");
    for item in items {
        html.push_str("<li>");
        html.push_str(&item);
        html.push_str("</li>");
    }
    html.push_str("</ul>");
    html
}
"#,
    );

    create_temp_file(
        &dir,
        "file2.rs",
        r"
fn compute_stats(numbers: Vec<f64>) -> (f64, f64) {
    let mut sum = 0.0;
    let mut count = 0.0;
    for n in numbers {
        sum += n;
        count += 1.0;
    }
    let mean = sum / count;
    (mean, sum)
}
",
    );

    let config = DetectConfig {
        enable_type3: true,
        type3_threshold: 0.7,
        ..DetectConfig::default()
    };
    let result = scan_and_run(&dir, &config);

    assert_eq!(
        result.stats.type3_groups, 0,
        "Should not flag structurally different functions as instances, even with similar sizes"
    );
}

/// The exact pattern from the proxy.rs screenshot: two if-let blocks with
/// identical structure but different header constants. This should be detected
/// as a Type 2 clone (same structure, different identifiers).
#[test]
fn detects_if_let_conditional_header_pattern() {
    let dir = TempDir::new().unwrap();

    create_temp_file(
        &dir,
        "proxy.rs",
        r#"
use std::collections::HashMap;

fn build_conditional_headers(cached: &HashMap<String, String>) -> HashMap<String, String> {
    let mut headers = HashMap::new();

    if let Some(etag) = cached.get("etag")
    {
        headers.insert("if-none-match".to_string(), etag.clone());
    }

    if let Some(last_modified) = cached.get("last-modified")
    {
        headers.insert("if-modified-since".to_string(), last_modified.clone());
    }

    headers
}
"#,
    );

    let result = scan_and_run(&dir, &DetectConfig::default());

    let total = result.stats.type1_groups + result.stats.type2_groups;
    assert!(
        total > 0,
        "Should detect the two if-let blocks as Type 1 or Type 2 instances"
    );
}

/// Type 3 similarity with functions that share ~80% of their structure
/// but have different endings should score high.
#[test]
fn type3_high_overlap_detected() {
    let dir = TempDir::new().unwrap();

    create_temp_file(
        &dir,
        "file1.rs",
        r#"
fn handle_request(data: &str) -> Result<String, String> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Err("empty".to_string());
    }
    let parsed = trimmed.to_uppercase();
    let validated = parsed.len() > 2;
    if !validated {
        return Err("too short".to_string());
    }
    Ok(parsed)
}
"#,
    );

    create_temp_file(
        &dir,
        "file2.rs",
        r#"
fn process_input(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("empty".to_string());
    }
    let parsed = trimmed.to_lowercase();
    let validated = parsed.len() > 2;
    if !validated {
        return Err("too short".to_string());
    }
    Ok(format!("result: {}", parsed))
}
"#,
    );

    let config = DetectConfig {
        enable_type3: true,
        type3_threshold: 0.6,
        ..DetectConfig::default()
    };
    let result = scan_and_run(&dir, &config);

    let total = result.stats.type2_groups + result.stats.type3_groups;
    assert!(
        total > 0,
        "Functions sharing ~80% structure should be detected as instances"
    );
}
