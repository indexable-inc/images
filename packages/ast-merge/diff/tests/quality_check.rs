mod common;

use common::rust;

#[test]
fn test_add_statements_both_ends() {
    let base = r"fn process() {
    middle();
}
";
    let left = r"fn process() {
    left_first();
    middle();
}
";
    let right = r"fn process() {
    middle();
    right_last();
}
";

    let result = rust(base, left, right);
    println!("Output:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("left_first"),
        "Missing left's addition"
    );
    assert!(
        result.content.contains("right_last"),
        "Missing right's addition"
    );
}

#[test]
fn test_add_different_statements_at_same_position() {
    let base = r"fn process() {
    let x = 1;
}
";
    let left = r#"fn process() {
    let x = 1;
    let a = "left";
}
"#;
    let right = r#"fn process() {
    let x = 1;
    let b = "right";
}
"#;

    let result = rust(base, left, right);
    println!("Output:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(result.content.contains("let a"), "Missing left's addition");
    assert!(result.content.contains("let b"), "Missing right's addition");
}

#[test]
fn test_modify_first_and_last_lines() {
    let base = r"fn compute() {
    let a = 1;
    let b = 2;
    let c = 3;
}
";
    let left = r"fn compute() {
    let a = 100;
    let b = 2;
    let c = 3;
}
";
    let right = r"fn compute() {
    let a = 1;
    let b = 2;
    let c = 300;
}
";

    let result = rust(base, left, right);
    println!("Output:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("a = 100"),
        "Missing left's modification"
    );
    assert!(
        result.content.contains("c = 300"),
        "Missing right's modification"
    );
}

#[test]
fn test_struct_different_fields() {
    let base = r"struct Config {
    a: i32,
    b: i32,
    c: i32,
}
";
    let left = r"struct Config {
    a: i64,
    b: i32,
    c: i32,
}
";
    let right = r"struct Config {
    a: i32,
    b: i32,
    c: i64,
}
";

    let result = rust(base, left, right);
    println!("Output:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("a: i64"),
        "Missing left's field change"
    );
    assert!(
        result.content.contains("c: i64"),
        "Missing right's field change"
    );
}

#[test]
fn test_impl_different_methods_modified() {
    let base = r#"impl Server {
    fn start(&self) {
        println!("start");
    }
    fn stop(&self) {
        println!("stop");
    }
}
"#;
    let left = r#"impl Server {
    fn start(&self) {
        println!("LEFT start");
    }
    fn stop(&self) {
        println!("stop");
    }
}
"#;
    let right = r#"impl Server {
    fn start(&self) {
        println!("start");
    }
    fn stop(&self) {
        println!("RIGHT stop");
    }
}
"#;

    let result = rust(base, left, right);
    println!("Output:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("LEFT start"),
        "Missing left's method change"
    );
    assert!(
        result.content.contains("RIGHT stop"),
        "Missing right's method change"
    );
}

#[test]
fn test_both_add_imports() {
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
    println!("Output:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("use std::fs"),
        "Missing left's import"
    );
    assert!(
        result.content.contains("use std::path"),
        "Missing right's import"
    );

    let fs_pos = result.content.find("use std::fs").unwrap();
    let path_pos = result.content.find("use std::path").unwrap();
    let main_pos = result.content.find("fn main").unwrap();
    assert!(
        fs_pos < main_pos,
        "fs import should be before main, got:\n{}",
        result.content
    );
    assert!(
        path_pos < main_pos,
        "path import should be before main, got:\n{}",
        result.content
    );
}

#[test]
fn test_one_deletes_one_modifies_function() {
    let base = r#"fn keep() {}

fn disputed() {
    println!("original");
}
"#;
    let left = r"fn keep() {}
";
    let right = r#"fn keep() {}

fn disputed() {
    println!("modified by right");
}
"#;

    let result = rust(base, left, right);
    println!("One deletes one modifies:\n{}", result.content);
}

#[test]
fn test_nested_impl_method_body_changes() {
    let base = r"impl Parser {
    fn parse(&mut self) {
        self.init();
        self.run();
        self.cleanup();
    }
}
";
    let left = r"impl Parser {
    fn parse(&mut self) {
        self.init();
        self.left_work();
        self.run();
        self.cleanup();
    }
}
";
    let right = r"impl Parser {
    fn parse(&mut self) {
        self.init();
        self.run();
        self.right_work();
        self.cleanup();
    }
}
";

    let result = rust(base, left, right);
    println!("Nested impl method changes:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("left_work"),
        "Missing left's addition:\n{}",
        result.content
    );
    assert!(
        result.content.contains("right_work"),
        "Missing right's addition:\n{}",
        result.content
    );
}

#[test]
fn test_enum_variant_changes() {
    let base = r"enum Status {
    Pending,
    Active,
    Done,
}
";
    let left = r"enum Status {
    Pending,
    Active,
    Done,
    LeftVariant,
}
";
    let right = r"enum Status {
    Pending,
    Active,
    Done,
    RightVariant,
}
";

    let result = rust(base, left, right);
    println!("Enum variant changes:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("LeftVariant"),
        "Missing left's variant:\n{}",
        result.content
    );
    assert!(
        result.content.contains("RightVariant"),
        "Missing right's variant:\n{}",
        result.content
    );
}

#[test]
fn test_trait_impl_different_methods() {
    let base = r"trait Worker {
    fn init(&self);
    fn run(&self);
}
";
    let left = r"trait Worker {
    fn init(&self);
    fn run(&self);
    fn left_method(&self);
}
";
    let right = r"trait Worker {
    fn init(&self);
    fn run(&self);
    fn right_method(&self);
}
";

    let result = rust(base, left, right);
    println!("Trait method changes:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("left_method"),
        "Missing left's method:\n{}",
        result.content
    );
    assert!(
        result.content.contains("right_method"),
        "Missing right's method:\n{}",
        result.content
    );
}

#[test]
fn test_multiline_statement_changes() {
    let base = r"fn complex() {
    let result = compute()
        .with_option_a()
        .with_option_b();
}
";
    let left = r"fn complex() {
    let result = compute()
        .with_option_a()
        .with_left_option()
        .with_option_b();
}
";
    let right = r"fn complex() {
    let result = compute()
        .with_option_a()
        .with_option_b()
        .with_right_option();
}
";

    let result = rust(base, left, right);
    println!("Multiline statement changes:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("with_left_option"),
        "Missing left's chain method:\n{}",
        result.content
    );
    assert!(
        result.content.contains("with_right_option"),
        "Missing right's chain method:\n{}",
        result.content
    );
}

#[test]
fn test_attribute_changes() {
    let base = r"fn example() {}
";
    let left = r"#[inline]
fn example() {}
";
    let right = r"#[must_use]
fn example() {}
";

    let result = rust(base, left, right);
    println!("Attribute changes:\n{}", result.content);
}

#[test]
fn test_comment_preservation() {
    let base = r"fn foo() {
    // Original comment
    do_something();
}
";
    let left = r"fn foo() {
    // Left's comment
    do_something();
}
";
    let right = r"fn foo() {
    // Right's comment
    do_something();
}
";

    let result = rust(base, left, right);
    println!("Comment changes:\n{}", result.content);
}

#[test]
fn test_complex_function_with_nested_blocks() {
    let base = r"fn handle_request() {
    if condition {
        setup();
    }
    process();
}
";
    let left = r"fn handle_request() {
    if condition {
        setup();
        left_init();
    }
    process();
}
";
    let right = r"fn handle_request() {
    if condition {
        setup();
    }
    process();
    right_cleanup();
}
";

    let result = rust(base, left, right);
    println!("Complex nested blocks:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("left_init"),
        "Missing left's nested addition:\n{}",
        result.content
    );
    assert!(
        result.content.contains("right_cleanup"),
        "Missing right's addition:\n{}",
        result.content
    );
}

#[test]
fn test_multiple_structs_different_changes() {
    let base = r"struct A {
    x: i32,
}

struct B {
    y: i32,
}
";
    let left = r"struct A {
    x: i64,
}

struct B {
    y: i32,
}
";
    let right = r"struct A {
    x: i32,
}

struct B {
    y: i64,
}
";

    let result = rust(base, left, right);
    println!("Multiple structs:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("x: i64"),
        "Missing left's change to A:\n{}",
        result.content
    );
    assert!(
        result.content.contains("y: i64"),
        "Missing right's change to B:\n{}",
        result.content
    );
}

#[test]
fn test_function_with_doc_comment_changes() {
    let base = r"/// Original doc
fn documented() {}
";
    let left = r"/// Left's doc
fn documented() {}
";
    let right = r"/// Right's doc
fn documented() {}
";

    let result = rust(base, left, right);
    println!("Doc comment changes:\n{}", result.content);
}

#[test]
fn test_generic_type_changes() {
    let base = r"struct Container<T> {
    data: T,
}
";
    let left = r"struct Container<T: Clone> {
    data: T,
}
";
    let right = r"struct Container<T> {
    data: T,
    count: usize,
}
";

    let result = rust(base, left, right);
    println!("Generic type changes:\n{}", result.content);
}

#[test]
fn test_verify_output_is_valid_rust() {
    let base = r"fn foo() {
    let x = 1;
}

fn bar() {
    let y = 2;
}
";
    let left = r"fn foo() {
    let x = 100;
}

fn bar() {
    let y = 2;
}

fn baz() {
    let z = 3;
}
";
    let right = r"fn foo() {
    let x = 1;
}

fn bar() {
    let y = 200;
}

fn qux() {
    let w = 4;
}
";

    let result = rust(base, left, right);
    assert!(result.success, "Should merge without conflict");

    let lang = ast_merge_langs::Lang::Rust.to_tree_sitter();
    let parsed = ast_merge_ast::tree(&result.content, &lang);
    assert!(
        parsed.is_ok(),
        "Output should be valid Rust: {}",
        result.content
    );
    let parsed = parsed.unwrap();
    assert!(
        !parsed.has_errors,
        "Output should have no parse errors:\n{}",
        result.content
    );

    assert!(result.content.contains("fn foo()"), "Missing foo");
    assert!(result.content.contains("fn bar()"), "Missing bar");
    assert!(result.content.contains("fn baz()"), "Missing baz");
    assert!(result.content.contains("fn qux()"), "Missing qux");
    assert!(
        result.content.contains("let x = 100"),
        "Missing left's change to foo"
    );
    assert!(
        result.content.contains("let y = 200"),
        "Missing right's change to bar"
    );
}

#[test]
fn test_add_and_modify_same_impl() {
    let base = r"impl Widget {
    fn render(&self) {
        draw();
    }
}
";
    let left = r"impl Widget {
    fn render(&self) {
        clear();
        draw();
    }
    fn on_click(&self) {
        handle();
    }
}
";
    let right = r"impl Widget {
    fn render(&self) {
        draw();
        flush();
    }
    fn on_hover(&self) {
        highlight();
    }
}
";

    let result = rust(base, left, right);
    println!("Add and modify same impl:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");

    assert!(
        result.content.contains("clear"),
        "Missing left's modification:\n{}",
        result.content
    );
    assert!(
        result.content.contains("flush"),
        "Missing right's modification:\n{}",
        result.content
    );

    assert!(
        result.content.contains("on_click"),
        "Missing left's new method:\n{}",
        result.content
    );
    assert!(
        result.content.contains("on_hover"),
        "Missing right's new method:\n{}",
        result.content
    );
}

#[test]
fn test_both_add_methods_to_impl() {
    let base = r"impl Foo {
    fn existing(&self) {}
}
";
    let left = r#"impl Foo {
    fn existing(&self) {}
    fn left_method(&self) { println!("left"); }
}
"#;
    let right = r#"impl Foo {
    fn existing(&self) {}
    fn right_method(&self) { println!("right"); }
}
"#;

    let result = rust(base, left, right);
    println!("Output:\n{}", result.content);
    assert!(result.success, "Should merge without conflict");
    assert!(
        result.content.contains("left_method"),
        "Missing left's new method"
    );
    assert!(
        result.content.contains("right_method"),
        "Missing right's new method"
    );
}
