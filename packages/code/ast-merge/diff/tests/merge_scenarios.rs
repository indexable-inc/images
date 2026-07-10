mod common;

use common::{CleanMergeCase, assert_clean_merges, rust};

#[test]
fn independent_merge_scenarios() {
    let cases = [
        CleanMergeCase {
            name: "left modifies, right adds",
            base: r#"fn greet() { println!("Hello"); }"#,
            left: r#"fn greet() { println!("Hello, World!"); }"#,
            right: r#"fn greet() { println!("Hello"); }
                fn farewell() { println!("Goodbye"); }"#,
            expected: &["Hello, World!", "fn farewell()"],
        },
        CleanMergeCase {
            name: "right modifies, left adds",
            base: "fn process() { do_work(); }",
            left: "fn process() { do_work(); } fn helper() { assist(); }",
            right: "fn process() { do_more_work(); }",
            expected: &["do_more_work", "fn helper()"],
        },
        CleanMergeCase {
            name: "both add functions",
            base: "fn main() { run(); }",
            left: r#"fn main() { run(); } fn feature_a() { println!("Feature A"); }"#,
            right: r#"fn main() { run(); } fn feature_b() { println!("Feature B"); }"#,
            expected: &["fn feature_a()", "fn feature_b()"],
        },
        CleanMergeCase {
            name: "multiple mixed changes",
            base: r#"fn alpha() { println!("alpha"); }
                fn beta() { println!("beta"); }"#,
            left: r#"fn alpha() { println!("ALPHA MODIFIED"); }
                fn beta() { println!("beta"); }
                fn gamma() { println!("gamma from left"); }"#,
            right: r#"fn alpha() { println!("alpha"); }
                fn beta() { println!("BETA MODIFIED"); }
                fn delta() { println!("delta from right"); }"#,
            expected: &["ALPHA MODIFIED", "BETA MODIFIED", "fn gamma()", "fn delta()"],
        },
        CleanMergeCase {
            name: "different lines in one function",
            base: "fn configure() { let timeout = 30; let retries = 3; let verbose = false; }",
            left: "fn configure() { let timeout = 60; let retries = 3; let verbose = false; }",
            right: "fn configure() { let timeout = 30; let retries = 3; let verbose = true; }",
            expected: &["timeout = 60", "verbose = true"],
        },
        CleanMergeCase {
            name: "different struct fields",
            base: "struct Settings { name: String, count: u32, enabled: bool }",
            left: "struct Settings { name: String, count: u64, enabled: bool }",
            right: "struct Settings { name: String, count: u32, enabled: Option<bool> }",
            expected: &["count: u64", "enabled: Option<bool>"],
        },
        CleanMergeCase {
            name: "different impl methods",
            base: r#"impl Server {
                fn start(&self) { println!("starting"); }
                fn stop(&self) { println!("stopping"); }
            }"#,
            left: r#"impl Server {
                fn start(&self) { println!("starting server..."); }
                fn stop(&self) { println!("stopping"); }
            }"#,
            right: r#"impl Server {
                fn start(&self) { println!("starting"); }
                fn stop(&self) { println!("gracefully stopping..."); }
            }"#,
            expected: &["starting server", "gracefully stopping"],
        },
        CleanMergeCase {
            name: "different imports",
            base: "use std::io; fn main() {}",
            left: "use std::io; use std::fs; fn main() {}",
            right: "use std::io; use std::path; fn main() {}",
            expected: &["use std::fs", "use std::path"],
        },
    ];

    assert_clean_merges(&cases);
}

#[test]
fn identical_changes_are_not_duplicated() {
    let base = "fn calculate() -> i32 { 1 + 1 }";
    let changed = "fn calculate() -> i32 { 2 + 2 }";
    let result = rust(base, changed, changed);
    assert!(result.success);
    assert!(result.content.contains("2 + 2"));
    assert_eq!(result.content.matches("fn calculate()").count(), 1);
}
