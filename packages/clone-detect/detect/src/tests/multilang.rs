use tempfile::TempDir;

use super::helpers::{create_temp_file, scan_and_run};
use crate::DetectConfig;

#[test]
fn mixed_language() {
    let dir = TempDir::new().unwrap();

    let ts_code = r"
function calculateSum(a: number, b: number): number {
    const result = a + b;
    return result;
}
";

    let js_code = r"
function calculateSum(a, b) {
    const result = a + b;
    return result;
}
";
    create_temp_file(&dir, "file1.ts", ts_code);
    create_temp_file(&dir, "file2.ts", ts_code);
    create_temp_file(&dir, "file3.js", js_code);

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert!(result.stats.type1_groups > 0 || result.stats.type2_groups > 0);
}

#[test]
fn python_duplicates() {
    let dir = TempDir::new().unwrap();

    let code = r"
def calculate_sum(a, b):
    result = a + b
    return result
";
    create_temp_file(&dir, "file1.py", code);
    create_temp_file(&dir, "file2.py", code);

    let result = scan_and_run(&dir, &DetectConfig::default());

    assert!(
        result.stats.type1_groups > 0,
        "Should detect Python duplicates"
    );
}
