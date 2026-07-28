use super::rust;

#[test]
fn independent_edits_merge() {
    let cases: &[(&str, &str, &str, &str, &[&str])] = &[
        (
            "statements in one function",
            "fn process() {\n    let x = 1;\n}\n",
            "fn process() {\n    let x = 1;\n    let left_var = \"from left\";\n}\n",
            "fn process() {\n    let x = 1;\n    let right_var = \"from right\";\n}\n",
            &["left_var", "right_var"],
        ),
        (
            "different struct fields",
            "struct Config {\n    name: String,\n    value: i32,\n    enabled: bool,\n}\n",
            "struct Config {\n    name: String,\n    value: i64,\n    enabled: bool,\n}\n",
            "struct Config {\n    name: String,\n    value: i32,\n    enabled: Option<bool>,\n}\n",
            &["value: i64", "enabled: Option<bool>"],
        ),
        (
            "different methods",
            "impl Foo {\n    fn method_a(&self) { println!(\"a\"); }\n    fn method_b(&self) { println!(\"b\"); }\n}\n",
            "impl Foo {\n    fn method_a(&self) { println!(\"a modified by left\"); }\n    fn method_b(&self) { println!(\"b\"); }\n}\n",
            "impl Foo {\n    fn method_a(&self) { println!(\"a\"); }\n    fn method_b(&self) { println!(\"b modified by right\"); }\n}\n",
            &["a modified by left", "b modified by right"],
        ),
        (
            "imports",
            "use std::io;\n\nfn main() {}\n",
            "use std::io;\nuse std::fs;\n\nfn main() {}\n",
            "use std::io;\nuse std::path;\n\nfn main() {}\n",
            &["use std::fs;", "use std::path;"],
        ),
    ];

    for &(name, base, left, right, expected) in cases {
        let result = rust(base, left, right);
        assert!(result.success, "{name}: {}", result.content);
        for needle in expected {
            assert!(
                result.content.contains(needle),
                "{name}: missing {needle:?} in {}",
                result.content
            );
        }
    }
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
