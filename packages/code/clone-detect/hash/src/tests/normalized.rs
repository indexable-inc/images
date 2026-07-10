use super::helpers::{pair_hashes, parse_js, parse_python, parse_rust};
use crate::{Dual, dual, hash as normalized};

type Parser = fn(&str) -> ast_merge_ast::Tree;

#[test]
fn normalized_hash_relations() {
    let cases: &[(&str, Parser, &str, &str, bool)] = &[
        (
            "renamed functions",
            parse_rust,
            "fn foo() { let x = 1; }",
            "fn bar() { let y = 1; }",
            true,
        ),
        (
            "different structure",
            parse_rust,
            "fn foo() { let x = 1; }",
            "fn foo() { let x = 1; let y = 2; }",
            false,
        ),
        (
            "swapped identifiers",
            parse_rust,
            "fn f() { a + b }",
            "fn f() { b + a }",
            true,
        ),
        (
            "different operators",
            parse_rust,
            "fn f() { a + b }",
            "fn f() { a - b }",
            false,
        ),
        (
            "inconsistent identifier mapping",
            parse_rust,
            "fn f() { x + x }",
            "fn f() { x + y }",
            false,
        ),
        (
            "different argument counts",
            parse_rust,
            "fn f(a: i32) { a }",
            "fn f(a: i32, b: i32) { a }",
            false,
        ),
        (
            "different types",
            parse_rust,
            "fn f(a: i32) { a }",
            "fn f(a: i64) { a }",
            false,
        ),
        (
            "JavaScript rename",
            parse_js,
            "function add(a, b) { return a + b; }",
            "function sum(x, y) { return x + y; }",
            true,
        ),
        (
            "Python rename",
            parse_python,
            "def add(a, b):\n    return a + b",
            "def sum(x, y):\n    return x + y",
            true,
        ),
        (
            "closure rename",
            parse_rust,
            "fn f() { let add = |a, b| a + b; }",
            "fn g() { let sum = |x, y| x + y; }",
            true,
        ),
        (
            "complex function rename",
            parse_rust,
            r"fn calculate(a: i32, b: i32) -> i32 {
                let sum = a + b;
                let product = a * b;
                sum + product
            }",
            r"fn compute(x: i32, y: i32) -> i32 {
                let total = x + y;
                let result = x * y;
                total + result
            }",
            true,
        ),
        (
            "nested function rename",
            parse_rust,
            "fn outer() { fn inner() { let x = 1; } }",
            "fn wrapper() { fn nested() { let y = 1; } }",
            true,
        ),
        (
            "recursive function rename",
            parse_rust,
            "fn factorial(n: i32) -> i32 { if n <= 1 { 1 } else { n * factorial(n - 1) } }",
            "fn fact(x: i32) -> i32 { if x <= 1 { 1 } else { x * fact(x - 1) } }",
            true,
        ),
    ];

    for &(name, parse, left, right, equal) in cases {
        let (left, right) = pair_hashes(parse, normalized, left, right);
        assert_eq!(left == right, equal, "{name}");
    }
}

#[test]
fn dual_returns_both() {
    let tree = parse_rust("fn foo() { let x = 1; }");
    let Dual {
        content,
        normalized,
    } = dual(&tree, tree.root_node());
    assert_ne!(content, 0);
    assert_ne!(normalized, 0);
}

#[test]
fn dual_separates_content_from_normalized_identity() {
    let left = parse_rust("fn foo() { let x = 1; }");
    let right = parse_rust("fn bar() { let y = 1; }");
    let left = dual(&left, left.root_node());
    let right = dual(&right, right.root_node());
    assert_ne!(left.content, right.content);
    assert_eq!(left.normalized, right.normalized);
}

#[test]
fn empty_function_has_a_hash() {
    let tree = parse_rust("fn empty() {}");
    assert_ne!(normalized(&tree, tree.root_node()), 0);
}

#[test]
fn normalized_hash_is_deterministic() {
    let tree = parse_rust("fn foo() { let x = 1; let y = 2; x + y }");
    let expected = normalized(&tree, tree.root_node());
    for _ in 0..2 {
        assert_eq!(normalized(&tree, tree.root_node()), expected);
    }
}
