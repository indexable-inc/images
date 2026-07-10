mod common;

use common::{CleanMergeCase, assert_clean_merges, rust};

#[test]
fn basic_independent_edits() {
    assert_clean_merges(&[
        CleanMergeCase {
            name: "statements at both ends",
            base: "fn process() { middle(); }",
            left: "fn process() { left_first(); middle(); }",
            right: "fn process() { middle(); right_last(); }",
            expected: &["left_first", "right_last"],
        },
        CleanMergeCase {
            name: "statements at one position",
            base: "fn process() { let x = 1; }",
            left: "fn process() { let x = 1; let a = \"left\"; }",
            right: "fn process() { let x = 1; let b = \"right\"; }",
            expected: &["let a", "let b"],
        },
        CleanMergeCase {
            name: "first and last lines",
            base: "fn compute() { let a = 1; let b = 2; let c = 3; }",
            left: "fn compute() { let a = 100; let b = 2; let c = 3; }",
            right: "fn compute() { let a = 1; let b = 2; let c = 300; }",
            expected: &["a = 100", "c = 300"],
        },
        CleanMergeCase {
            name: "different struct fields",
            base: "struct Config { a: i32, b: i32, c: i32 }",
            left: "struct Config { a: i64, b: i32, c: i32 }",
            right: "struct Config { a: i32, b: i32, c: i64 }",
            expected: &["a: i64", "c: i64"],
        },
        CleanMergeCase {
            name: "different impl methods",
            base: r#"impl Server {
                fn start(&self) { println!("start"); }
                fn stop(&self) { println!("stop"); }
            }"#,
            left: r#"impl Server {
                fn start(&self) { println!("LEFT start"); }
                fn stop(&self) { println!("stop"); }
            }"#,
            right: r#"impl Server {
                fn start(&self) { println!("start"); }
                fn stop(&self) { println!("RIGHT stop"); }
            }"#,
            expected: &["LEFT start", "RIGHT stop"],
        },
    ]);
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
fn structural_independent_edits() {
    assert_clean_merges(&[
        CleanMergeCase {
            name: "nested method body",
            base: "impl Parser { fn parse(&mut self) { self.init(); self.run(); self.cleanup(); } }",
            left: "impl Parser { fn parse(&mut self) { self.init(); self.left_work(); self.run(); self.cleanup(); } }",
            right: "impl Parser { fn parse(&mut self) { self.init(); self.run(); self.right_work(); self.cleanup(); } }",
            expected: &["left_work", "right_work"],
        },
        CleanMergeCase {
            name: "enum variants",
            base: "enum Status { Pending, Active, Done }",
            left: "enum Status { Pending, Active, Done, LeftVariant }",
            right: "enum Status { Pending, Active, Done, RightVariant }",
            expected: &["LeftVariant", "RightVariant"],
        },
        CleanMergeCase {
            name: "trait methods",
            base: "trait Worker { fn init(&self); fn run(&self); }",
            left: "trait Worker { fn init(&self); fn run(&self); fn left_method(&self); }",
            right: "trait Worker { fn init(&self); fn run(&self); fn right_method(&self); }",
            expected: &["left_method", "right_method"],
        },
        CleanMergeCase {
            name: "method chain",
            base: "fn complex() { let result = compute().with_option_a().with_option_b(); }",
            left: "fn complex() { let result = compute().with_option_a().with_left_option().with_option_b(); }",
            right: "fn complex() { let result = compute().with_option_a().with_option_b().with_right_option(); }",
            expected: &["with_left_option", "with_right_option"],
        },
        CleanMergeCase {
            name: "nested blocks",
            base: "fn handle_request() { if condition { setup(); } process(); }",
            left: "fn handle_request() { if condition { setup(); left_init(); } process(); }",
            right: "fn handle_request() { if condition { setup(); } process(); right_cleanup(); }",
            expected: &["left_init", "right_cleanup"],
        },
        CleanMergeCase {
            name: "different structs",
            base: "struct A { x: i32 } struct B { y: i32 }",
            left: "struct A { x: i64 } struct B { y: i32 }",
            right: "struct A { x: i32 } struct B { y: i64 }",
            expected: &["x: i64", "y: i64"],
        },
    ]);
}

#[test]
fn edge_case_merges_have_explicit_outcomes() {
    assert_clean_merges(&[
        CleanMergeCase {
            name: "delete versus modify",
            base: r#"fn keep() {} fn disputed() { println!("original"); }"#,
            left: "fn keep() {}",
            right: r#"fn keep() {} fn disputed() { println!("modified by right"); }"#,
            expected: &["fn keep()"],
        },
        CleanMergeCase {
            name: "attributes",
            base: "fn example() {}",
            left: "#[inline] fn example() {}",
            right: "#[must_use] fn example() {}",
            expected: &["#[inline]", "#[must_use]"],
        },
        CleanMergeCase {
            name: "comments",
            base: "fn foo() { // Original comment\n do_something(); }",
            left: "fn foo() { // Left's comment\n do_something(); }",
            right: "fn foo() { // Right's comment\n do_something(); }",
            expected: &["Left's comment", "do_something"],
        },
        CleanMergeCase {
            name: "doc comments",
            base: "/// Original doc\nfn documented() {}",
            left: "/// Left's doc\nfn documented() {}",
            right: "/// Right's doc\nfn documented() {}",
            expected: &["fn documented()"],
        },
        CleanMergeCase {
            name: "generic bound and field",
            base: "struct Container<T> { data: T }",
            left: "struct Container<T: Clone> { data: T }",
            right: "struct Container<T> { data: T, count: usize }",
            expected: &["T: Clone", "count: usize"],
        },
    ]);
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
