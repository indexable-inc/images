mod common;

use common::rust;

#[test]
fn left_modifies_function_right_adds_new_function() {
    let base = r#"fn greet() {
    println!("Hello");
}
"#;

    let left = r#"fn greet() {
    println!("Hello, World!");
}
"#;

    let right = r#"fn greet() {
    println!("Hello");
}

fn farewell() {
    println!("Goodbye");
}
"#;

    let result = rust(base, left, right);

    assert!(result.success);
    assert!(
        result.content.contains("Hello, World!"),
        "left's modification preserved"
    );
    assert!(
        result.content.contains("fn farewell()"),
        "right's addition preserved"
    );
}

#[test]
fn right_modifies_function_left_adds_new_function() {
    let base = r"fn process() {
    do_work();
}
";

    let left = r"fn process() {
    do_work();
}

fn helper() {
    assist();
}
";

    let right = r"fn process() {
    do_more_work();
}
";

    let result = rust(base, left, right);

    assert!(result.success);
    assert!(
        result.content.contains("do_more_work"),
        "right's modification preserved"
    );
    assert!(
        result.content.contains("fn helper()"),
        "left's addition preserved"
    );
}

#[test]
fn both_add_different_functions() {
    let base = r"fn main() {
    run();
}
";

    let left = r#"fn main() {
    run();
}

fn feature_a() {
    println!("Feature A");
}
"#;

    let right = r#"fn main() {
    run();
}

fn feature_b() {
    println!("Feature B");
}
"#;

    let result = rust(base, left, right);

    assert!(result.success);
    assert!(
        result.content.contains("fn feature_a()"),
        "left's function preserved"
    );
    assert!(
        result.content.contains("fn feature_b()"),
        "right's function preserved"
    );
}

#[test]
fn both_make_identical_changes() {
    let base = r"fn calculate() -> i32 {
    1 + 1
}
";

    let left = r"fn calculate() -> i32 {
    2 + 2
}
";

    let right = r"fn calculate() -> i32 {
    2 + 2
}
";

    let result = rust(base, left, right);

    assert!(result.success);
    assert!(result.content.contains("2 + 2"), "change preserved");
    assert_eq!(
        result.content.matches("fn calculate()").count(),
        1,
        "no duplicate functions"
    );
}

#[test]
fn multiple_functions_mixed_changes() {
    let base = r#"fn alpha() {
    println!("alpha");
}

fn beta() {
    println!("beta");
}
"#;

    let left = r#"fn alpha() {
    println!("ALPHA MODIFIED");
}

fn beta() {
    println!("beta");
}

fn gamma() {
    println!("gamma from left");
}
"#;

    let right = r#"fn alpha() {
    println!("alpha");
}

fn beta() {
    println!("BETA MODIFIED");
}

fn delta() {
    println!("delta from right");
}
"#;

    let result = rust(base, left, right);

    assert!(result.success);
    assert!(
        result.content.contains("ALPHA MODIFIED"),
        "left's alpha change"
    );
    assert!(
        result.content.contains("BETA MODIFIED"),
        "right's beta change"
    );
    assert!(result.content.contains("fn gamma()"), "left's new function");
    assert!(
        result.content.contains("fn delta()"),
        "right's new function"
    );
}

#[test]
fn different_lines_same_function() {
    let base = r"fn configure() {
    let timeout = 30;
    let retries = 3;
    let verbose = false;
}
";

    let left = r"fn configure() {
    let timeout = 60;
    let retries = 3;
    let verbose = false;
}
";

    let right = r"fn configure() {
    let timeout = 30;
    let retries = 3;
    let verbose = true;
}
";

    let result = rust(base, left, right);

    assert!(
        result.success,
        "should merge without conflicts: {}",
        result.content
    );
    assert!(
        result.content.contains("timeout = 60"),
        "left's timeout change"
    );
    assert!(
        result.content.contains("verbose = true"),
        "right's verbose change"
    );
}

#[test]
fn struct_different_field_changes() {
    let base = r"struct Settings {
    name: String,
    count: u32,
    enabled: bool,
}
";

    let left = r"struct Settings {
    name: String,
    count: u64,
    enabled: bool,
}
";

    let right = r"struct Settings {
    name: String,
    count: u32,
    enabled: Option<bool>,
}
";

    let result = rust(base, left, right);

    assert!(
        result.success,
        "should merge struct changes: {}",
        result.content
    );
    assert!(
        result.content.contains("count: u64"),
        "left's field type change"
    );
    assert!(
        result.content.contains("enabled: Option<bool>"),
        "right's field type change"
    );
}

#[test]
fn impl_block_different_methods() {
    let base = r#"impl Server {
    fn start(&self) {
        println!("starting");
    }

    fn stop(&self) {
        println!("stopping");
    }
}
"#;

    let left = r#"impl Server {
    fn start(&self) {
        println!("starting server...");
    }

    fn stop(&self) {
        println!("stopping");
    }
}
"#;

    let right = r#"impl Server {
    fn start(&self) {
        println!("starting");
    }

    fn stop(&self) {
        println!("gracefully stopping...");
    }
}
"#;

    let result = rust(base, left, right);

    assert!(
        result.success,
        "should merge impl changes: {}",
        result.content
    );
    assert!(
        result.content.contains("starting server"),
        "left's start change"
    );
    assert!(
        result.content.contains("gracefully stopping"),
        "right's stop change"
    );
}

#[test]
fn both_add_different_imports() {
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

    assert!(result.success, "should merge imports: {}", result.content);
    assert!(result.content.contains("use std::fs"), "left's import");
    assert!(result.content.contains("use std::path"), "right's import");
}
