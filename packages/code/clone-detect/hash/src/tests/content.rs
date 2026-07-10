use super::helpers::{pair_hashes, parse_js, parse_rust};
use crate::compute;

#[test]
fn content_hash_relations() {
    let cases = [
        (
            "identical Rust",
            parse_rust as fn(&str) -> ast_merge_ast::Tree,
            "fn foo() { let x = 1; }",
            "fn foo() { let x = 1; }",
            true,
        ),
        (
            "renamed Rust",
            parse_rust,
            "fn foo() { let x = 1; }",
            "fn bar() { let y = 1; }",
            false,
        ),
        (
            "Rust whitespace",
            parse_rust,
            "fn foo(){let x=1;}",
            "fn foo() {\n    let x = 1;\n}",
            true,
        ),
        (
            "Rust literals",
            parse_rust,
            "fn f() { let x = 1; }",
            "fn f() { let x = 2; }",
            false,
        ),
        (
            "renamed JavaScript",
            parse_js,
            "function add(a, b) { return a + b; }",
            "function sum(x, y) { return x + y; }",
            false,
        ),
    ];
    for (name, parse, left, right, equal) in cases {
        let (left, right) = pair_hashes(parse, compute, left, right);
        assert_eq!(left == right, equal, "{name}");
    }
}
