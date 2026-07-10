use super::helpers::{HashPair, pair_hashes, parse_js, parse_python, parse_rust};
use crate::{Dual, dual, hash as normalized};

type Parser = fn(&str) -> ast_merge_ast::Tree;

struct Relation {
    name: &'static str,
    parse: Parser,
    left: &'static str,
    right: &'static str,
    equal: bool,
}

const RELATIONS: &[Relation] = &[
    Relation {
        name: "renamed functions",
        parse: parse_rust,
        left: "fn foo() { let x = 1; }",
        right: "fn bar() { let y = 1; }",
        equal: true,
    },
    Relation {
        name: "different structure",
        parse: parse_rust,
        left: "fn foo() { let x = 1; }",
        right: "fn foo() { let x = 1; let y = 2; }",
        equal: false,
    },
    Relation {
        name: "swapped identifiers",
        parse: parse_rust,
        left: "fn f() { a + b }",
        right: "fn f() { b + a }",
        equal: true,
    },
    Relation {
        name: "different operators",
        parse: parse_rust,
        left: "fn f() { a + b }",
        right: "fn f() { a - b }",
        equal: false,
    },
    Relation {
        name: "inconsistent identifier mapping",
        parse: parse_rust,
        left: "fn f() { x + x }",
        right: "fn f() { x + y }",
        equal: false,
    },
    Relation {
        name: "different argument counts",
        parse: parse_rust,
        left: "fn f(a: i32) { a }",
        right: "fn f(a: i32, b: i32) { a }",
        equal: false,
    },
    Relation {
        name: "different types",
        parse: parse_rust,
        left: "fn f(a: i32) { a }",
        right: "fn f(a: i64) { a }",
        equal: false,
    },
    Relation {
        name: "JavaScript rename",
        parse: parse_js,
        left: "function add(a, b) { return a + b; }",
        right: "function sum(x, y) { return x + y; }",
        equal: true,
    },
    Relation {
        name: "Python rename",
        parse: parse_python,
        left: "def add(a, b):\n    return a + b",
        right: "def sum(x, y):\n    return x + y",
        equal: true,
    },
    Relation {
        name: "closure rename",
        parse: parse_rust,
        left: "fn f() { let add = |a, b| a + b; }",
        right: "fn g() { let sum = |x, y| x + y; }",
        equal: true,
    },
    Relation {
        name: "complex function rename",
        parse: parse_rust,
        left: r"fn calculate(a: i32, b: i32) -> i32 {
                let sum = a + b;
                let product = a * b;
                sum + product
            }",
        right: r"fn compute(x: i32, y: i32) -> i32 {
                let total = x + y;
                let result = x * y;
                total + result
            }",
        equal: true,
    },
    Relation {
        name: "nested function rename",
        parse: parse_rust,
        left: "fn outer() { fn inner() { let x = 1; } }",
        right: "fn wrapper() { fn nested() { let y = 1; } }",
        equal: true,
    },
    Relation {
        name: "recursive function rename",
        parse: parse_rust,
        left: "fn factorial(n: i32) -> i32 { if n <= 1 { 1 } else { n * factorial(n - 1) } }",
        right: "fn fact(x: i32) -> i32 { if x <= 1 { 1 } else { x * fact(x - 1) } }",
        equal: true,
    },
];

#[test]
fn normalized_hash_relations() {
    for relation in RELATIONS {
        let HashPair { left, right } = pair_hashes(
            relation.parse,
            normalized,
            relation.left,
            relation.right,
        );
        assert_eq!(left == right, relation.equal, "{}", relation.name);
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
