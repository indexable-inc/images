use ast_merge_langs::Lang;

use super::helpers::{parse, parse_rust};
use crate::{Dual, dual, hash as normalized};

fn assert_hash_relations(cases: &[(&str, Lang, &str, &str, bool)]) {
    for &(name, lang, left, right, equal) in cases {
        let (left, right) = (parse(lang, left), parse(lang, right));
        let left = normalized(&left, lang, left.root_node());
        let right = normalized(&right, lang, right.root_node());
        assert_eq!(left == right, equal, "{name}");
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the cases form one readable normalization behavior table"
)]
fn normalized_hash_relations() {
    assert_hash_relations(&[
        (
            "renamed functions",
            Lang::Rust,
            "fn foo() { let x = 1; }",
            "fn bar() { let y = 1; }",
            true,
        ),
        (
            "different structure",
            Lang::Rust,
            "fn foo() { let x = 1; }",
            "fn foo() { let x = 1; let y = 2; }",
            false,
        ),
        (
            "swapped identifiers",
            Lang::Rust,
            "fn f() { a + b }",
            "fn f() { b + a }",
            true,
        ),
        (
            "different operators",
            Lang::Rust,
            "fn f() { a + b }",
            "fn f() { a - b }",
            false,
        ),
        (
            "inconsistent identifier mapping",
            Lang::Rust,
            "fn f() { x + x }",
            "fn f() { x + y }",
            false,
        ),
        (
            "different argument counts",
            Lang::Rust,
            "fn f(a: i32) { a }",
            "fn f(a: i32, b: i32) { a }",
            false,
        ),
        (
            "different types",
            Lang::Rust,
            "fn f(a: i32) { a }",
            "fn f(a: i64) { a }",
            false,
        ),
        (
            "JavaScript rename",
            Lang::JavaScript,
            "function add(a, b) { return a + b; }",
            "function sum(x, y) { return x + y; }",
            true,
        ),
        (
            "Python rename",
            Lang::Python,
            "def add(a, b):\n    return a + b",
            "def sum(x, y):\n    return x + y",
            true,
        ),
        (
            "closure rename",
            Lang::Rust,
            "fn f() { let add = |a, b| a + b; }",
            "fn g() { let sum = |x, y| x + y; }",
            true,
        ),
        (
            "complex function rename",
            Lang::Rust,
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
            Lang::Rust,
            "fn outer() { fn inner() { let x = 1; } }",
            "fn wrapper() { fn nested() { let y = 1; } }",
            true,
        ),
        (
            "recursive function rename",
            Lang::Rust,
            "fn factorial(n: i32) -> i32 { if n <= 1 { 1 } else { n * factorial(n - 1) } }",
            "fn fact(x: i32) -> i32 { if x <= 1 { 1 } else { x * fact(x - 1) } }",
            true,
        ),
    ]);
}

/// Per-language canonical views (`canon`): Elixir pipes hash like plain
/// calls, and keyword/struct-literal field order is hashed order-insensitively
/// (issue #3878).
#[test]
fn canonicalized_hash_relations() {
    assert_hash_relations(&[
        (
            "elixir pipe hashes like plain call",
            Lang::Elixir,
            "x |> f(a)",
            "f(x, a)",
            true,
        ),
        (
            "elixir bare pipe stage",
            Lang::Elixir,
            "x |> f",
            "f(x)",
            true,
        ),
        (
            "elixir pipe chain",
            Lang::Elixir,
            "x |> f(a) |> g(b)",
            "g(f(x, a), b)",
            true,
        ),
        (
            "elixir qualified pipe",
            Lang::Elixir,
            "x |> Mod.f(a)",
            "Mod.f(x, a)",
            true,
        ),
        (
            "elixir map key order",
            Lang::Elixir,
            "%{name: x, age: y}",
            "%{age: y, name: x}",
            true,
        ),
        (
            "elixir struct field order",
            Lang::Elixir,
            "%User{name: x, age: y}",
            "%User{age: y, name: x}",
            true,
        ),
        (
            // Ordered data: only map/struct keyword pairs sort (#3885 review).
            "elixir trailing keyword arguments stay ordered",
            Lang::Elixir,
            "f(x, a: 1, b: 2)",
            "f(x, b: 2, a: 1)",
            false,
        ),
        (
            "elixir bare keyword lists stay ordered",
            Lang::Elixir,
            "[a: x, b: y]",
            "[b: y, a: x]",
            false,
        ),
        (
            "rust struct literal field order",
            Lang::Rust,
            "fn f() { let u = User { name: 1, age: 2 }; }",
            "fn f() { let u = User { age: 2, name: 1 }; }",
            true,
        ),
        (
            "elixir pipe arity still distinguishes",
            Lang::Elixir,
            "x |> f(a)",
            "f(x)",
            false,
        ),
        (
            "elixir different keys still distinguish",
            Lang::Elixir,
            "%{name: x, age: y}",
            "%{name: x, size: y}",
            false,
        ),
        (
            "elixir keyword vs positional argument",
            Lang::Elixir,
            "f(x, a: 1)",
            "f(x, 1)",
            false,
        ),
        (
            "rust different field values still distinguish",
            Lang::Rust,
            "fn f() { let u = User { name: a, age: b }; }",
            "fn f() { let u = User { name: a.c, age: b }; }",
            false,
        ),
    ]);
}

#[test]
fn dual_returns_both() {
    let tree = parse_rust("fn foo() { let x = 1; }");
    let Dual {
        content,
        normalized,
    } = dual(&tree, Lang::Rust, tree.root_node());
    assert_ne!(content, 0);
    assert_ne!(normalized, 0);
}

#[test]
fn dual_separates_content_from_normalized_identity() {
    let left = parse_rust("fn foo() { let x = 1; }");
    let right = parse_rust("fn bar() { let y = 1; }");
    let left = dual(&left, Lang::Rust, left.root_node());
    let right = dual(&right, Lang::Rust, right.root_node());
    assert_ne!(left.content, right.content);
    assert_eq!(left.normalized, right.normalized);
}

#[test]
fn empty_function_has_a_hash() {
    let tree = parse_rust("fn empty() {}");
    assert_ne!(normalized(&tree, Lang::Rust, tree.root_node()), 0);
}

#[test]
fn normalized_hash_is_deterministic() {
    let tree = parse_rust("fn foo() { let x = 1; let y = 2; x + y }");
    let expected = normalized(&tree, Lang::Rust, tree.root_node());
    for _ in 0..2 {
        assert_eq!(normalized(&tree, Lang::Rust, tree.root_node()), expected);
    }
}
