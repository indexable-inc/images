use super::helpers::{parse_js, parse_rust};
use crate::compute;

#[test]
fn identical_functions_same() {
    let source1 = "fn foo() { let x = 1; }";
    let source2 = "fn foo() { let x = 1; }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = compute(&tree1, tree1.root_node());
    let hash2 = compute(&tree2, tree2.root_node());

    assert_eq!(hash1, hash2);
}

#[test]
fn renamed_functions_different() {
    let source1 = "fn foo() { let x = 1; }";
    let source2 = "fn bar() { let y = 1; }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = compute(&tree1, tree1.root_node());
    let hash2 = compute(&tree2, tree2.root_node());

    assert_ne!(hash1, hash2);
}

#[test]
fn whitespace_difference_same() {
    let source1 = "fn foo(){let x=1;}";
    let source2 = "fn foo() {\n    let x = 1;\n}";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = compute(&tree1, tree1.root_node());
    let hash2 = compute(&tree2, tree2.root_node());

    assert_eq!(hash1, hash2);
}

#[test]
fn different_literals() {
    let source1 = "fn f() { let x = 1; }";
    let source2 = "fn f() { let x = 2; }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = compute(&tree1, tree1.root_node());
    let hash2 = compute(&tree2, tree2.root_node());

    assert_ne!(hash1, hash2);
}

#[test]
fn javascript_differs() {
    let source1 = "function add(a, b) { return a + b; }";
    let source2 = "function sum(x, y) { return x + y; }";

    let tree1 = parse_js(source1);
    let tree2 = parse_js(source2);

    let hash1 = compute(&tree1, tree1.root_node());
    let hash2 = compute(&tree2, tree2.root_node());

    assert_ne!(hash1, hash2);
}
