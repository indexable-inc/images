use super::rust;

#[test]
fn test_both_add_statements_same_function() {
    let base = r"fn process() {
    let x = 1;
}
";

    let left = r#"fn process() {
    let x = 1;
    let left_var = "from left";
}
"#;

    let right = r#"fn process() {
    let x = 1;
    let right_var = "from right";
}
"#;

    let result = rust(base, left, right);
    assert!(
        result.success,
        "should merge both additions to same function: {}",
        result.content
    );
    assert!(
        result.content.contains("left_var"),
        "should have left's addition: {}",
        result.content
    );
    assert!(
        result.content.contains("right_var"),
        "should have right's addition: {}",
        result.content
    );
}

#[test]
fn test_nested_struct_fields() {
    let base = r"struct Config {
    name: String,
    value: i32,
    enabled: bool,
}
";

    let left = r"struct Config {
    name: String,
    value: i64,
    enabled: bool,
}
";

    let right = r"struct Config {
    name: String,
    value: i32,
    enabled: Option<bool>,
}
";

    let result = rust(base, left, right);
    assert!(
        result.success,
        "should merge different struct field changes: {}",
        result.content
    );
    assert!(
        result.content.contains("value: i64"),
        "should have left's field change: {}",
        result.content
    );
    assert!(
        result.content.contains("enabled: Option<bool>"),
        "should have right's field change: {}",
        result.content
    );
}

#[test]
fn test_impl_block_different_methods() {
    let base = r#"impl Foo {
    fn method_a(&self) {
        println!("a");
    }

    fn method_b(&self) {
        println!("b");
    }
}
"#;

    let left = r#"impl Foo {
    fn method_a(&self) {
        println!("a modified by left");
    }

    fn method_b(&self) {
        println!("b");
    }
}
"#;

    let right = r#"impl Foo {
    fn method_a(&self) {
        println!("a");
    }

    fn method_b(&self) {
        println!("b modified by right");
    }
}
"#;

    let result = rust(base, left, right);
    assert!(
        result.success,
        "should merge different method changes in same impl: {}",
        result.content
    );
    assert!(
        result.content.contains("a modified by left"),
        "should have left's method_a change: {}",
        result.content
    );
    assert!(
        result.content.contains("b modified by right"),
        "should have right's method_b change: {}",
        result.content
    );
}

#[test]
fn test_reorderable_imports() {
    let base = r"use std::io;

fn main() {}
";

    let left = r"use std::io;
use std::fs;

fn main() {}
";

    let right = r"use std::io;
use std::path;

fn main() {}
";

    let result = rust(base, left, right);
    assert!(
        result.success,
        "should merge different import additions: {}",
        result.content
    );
    assert!(
        result.content.contains("use std::fs;"),
        "should have left's import: {}",
        result.content
    );
    assert!(
        result.content.contains("use std::path;"),
        "should have right's import: {}",
        result.content
    );
}

/// Regression test: when both sides modify the same function body differently,
/// the merge must produce conflict markers -- NOT silently pick one side.
/// Before the fix (ENG-466 partial), the merge silently concatenated both
/// sides' changes or picked left's version.
#[test]
fn test_conflicting_function_body_produces_conflict_markers() {
    let base = r#"fn greet(name: &str) {
    println!("hi {name}");
}
"#;
    let left = r#"fn greet(name: &str) {
    println!("good morning {name}");
}
"#;
    let right = r#"fn greet(name: &str) {
    println!("good evening {name}");
}
"#;

    let result = rust(base, left, right);
    // The structural detect_conflicts may or may not flag this (it detects
    // tree-structure conflicts, not content conflicts). But the item-level
    // merge must produce conflict markers in the content.
    assert!(
        result.content.contains("<<<<<<<") && result.content.contains(">>>>>>>"),
        "merge output must contain conflict markers when both sides modify the same function:\n{}",
        result.content
    );
    assert!(
        result.content.contains("good morning") && result.content.contains("good evening"),
        "conflict markers must show both sides' content:\n{}",
        result.content
    );
}
