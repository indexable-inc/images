use super::rust;

#[test]
fn test_left_modifies_right_adds() {
    let base = r#"fn original() {
    println!("original");
}
"#;

    let left = r#"fn original() {
    println!("modified by left");
}
"#;

    let right = r#"fn original() {
    println!("original");
}

fn right_new() {
    println!("from right");
}
"#;

    let result = rust(base, left, right);
    assert!(result.success, "merge should succeed without conflicts");

    assert!(
        result.content.contains("modified by left"),
        "should contain left's modification: {}",
        result.content
    );

    assert!(
        result.content.contains("fn right_new()"),
        "should contain right's new function: {}",
        result.content
    );
    assert!(
        result.content.contains("from right"),
        "should contain right's function body: {}",
        result.content
    );
}

#[test]
fn test_right_modifies_left_adds() {
    let base = r#"fn original() {
    println!("original");
}
"#;

    let left = r#"fn original() {
    println!("original");
}

fn left_new() {
    println!("from left");
}
"#;

    let right = r#"fn original() {
    println!("modified by right");
}
"#;

    let result = rust(base, left, right);
    assert!(result.success, "merge should succeed without conflicts");

    assert!(
        result.content.contains("modified by right"),
        "should contain right's modification: {}",
        result.content
    );

    assert!(
        result.content.contains("fn left_new()"),
        "should contain left's new function: {}",
        result.content
    );
    assert!(
        result.content.contains("from left"),
        "should contain left's function body: {}",
        result.content
    );
}

#[test]
fn test_both_add_different_functions() {
    let base = r#"fn original() {
    println!("original");
}
"#;

    let left = r#"fn original() {
    println!("original");
}

fn left_new() {
    println!("from left");
}
"#;

    let right = r#"fn original() {
    println!("original");
}

fn right_new() {
    println!("from right");
}
"#;

    let result = rust(base, left, right);
    assert!(result.success, "merge should succeed without conflicts");

    assert!(
        result.content.contains("fn left_new()"),
        "should contain left's new function: {}",
        result.content
    );
    assert!(
        result.content.contains("fn right_new()"),
        "should contain right's new function: {}",
        result.content
    );
}

#[test]
fn test_identical_changes() {
    let base = r#"fn original() {
    println!("original");
}
"#;

    let left = r#"fn original() {
    println!("same change");
}
"#;

    let right = r#"fn original() {
    println!("same change");
}
"#;

    let result = rust(base, left, right);
    assert!(result.success, "merge should succeed without conflicts");
    assert!(
        result.content.contains("same change"),
        "should contain the common change: {}",
        result.content
    );

    let count = result.content.matches("fn original()").count();
    assert_eq!(count, 1, "should have exactly one original function");
}

#[test]
fn test_no_changes() {
    let base = r#"fn original() {
    println!("original");
}
"#;

    let result = rust(base, base, base);
    assert!(result.success, "merge should succeed without conflicts");
    assert!(
        result.content.contains("fn original()"),
        "should preserve original: {}",
        result.content
    );
}

#[test]
fn test_multiple_functions_mixed_changes() {
    let base = r#"fn foo() {
    println!("foo");
}

fn bar() {
    println!("bar");
}
"#;

    let left = r#"fn foo() {
    println!("foo modified by left");
}

fn bar() {
    println!("bar");
}

fn baz() {
    println!("baz from left");
}
"#;

    let right = r#"fn foo() {
    println!("foo");
}

fn bar() {
    println!("bar modified by right");
}

fn qux() {
    println!("qux from right");
}
"#;

    let result = rust(base, left, right);
    assert!(result.success, "merge should succeed without conflicts");

    assert!(
        result.content.contains("foo modified by left"),
        "should have left's foo change: {}",
        result.content
    );

    assert!(
        result.content.contains("bar modified by right"),
        "should have right's bar change: {}",
        result.content
    );

    assert!(
        result.content.contains("fn baz()"),
        "should have left's new baz: {}",
        result.content
    );
    assert!(
        result.content.contains("fn qux()"),
        "should have right's new qux: {}",
        result.content
    );
}

#[test]
fn test_different_lines_same_function() {
    let base = r"fn process() {
    let a = 1;
    let b = 2;
    let c = 3;
}
";

    let left = r"fn process() {
    let a = 100;
    let b = 2;
    let c = 3;
}
";

    let right = r"fn process() {
    let a = 1;
    let b = 2;
    let c = 300;
}
";

    let result = rust(base, left, right);
    assert!(
        result.success,
        "should merge different lines in same function without conflict: {}",
        result.content
    );
    assert!(
        result.content.contains("let a = 100;"),
        "should have left's change to line 1: {}",
        result.content
    );
    assert!(
        result.content.contains("let c = 300;"),
        "should have right's change to line 3: {}",
        result.content
    );
}
