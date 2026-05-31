use super::helpers::{parse_js, parse_python, parse_rust};
use crate::{Dual, dual, hash as normalized};

#[test]
fn renamed_functions_same() {
    let source1 = "fn foo() { let x = 1; }";
    let source2 = "fn bar() { let y = 1; }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_eq!(hash1, hash2);
}

#[test]
fn different_structure_different() {
    let source1 = "fn foo() { let x = 1; }";
    let source2 = "fn foo() { let x = 1; let y = 2; }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_ne!(hash1, hash2);
}

#[test]
fn swapped_identifiers_same() {
    let source1 = "fn f() { a + b }";
    let source2 = "fn f() { b + a }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_eq!(hash1, hash2);
}

#[test]
fn different_operators_different() {
    let source1 = "fn f() { a + b }";
    let source2 = "fn f() { a - b }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_ne!(hash1, hash2);
}

#[test]
fn consistent_identifier_mapping() {
    let source1 = "fn f() { x + x }";
    let source2 = "fn f() { x + y }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_ne!(hash1, hash2);
}

#[test]
fn complex_function() {
    let source1 = r"
fn calculate(a: i32, b: i32) -> i32 {
    let sum = a + b;
    let product = a * b;
    sum + product
}
";
    let source2 = r"
fn compute(x: i32, y: i32) -> i32 {
    let total = x + y;
    let result = x * y;
    total + result
}
";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_eq!(hash1, hash2);
}

#[test]
fn different_arg_count() {
    let source1 = "fn f(a: i32) { a }";
    let source2 = "fn f(a: i32, b: i32) { a }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_ne!(hash1, hash2);
}

#[test]
fn different_types() {
    let source1 = "fn f(a: i32) { a }";
    let source2 = "fn f(a: i64) { a }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_ne!(hash1, hash2);
}

#[test]
fn dual_returns_both() {
    let source = "fn foo() { let x = 1; }";
    let tree = parse_rust(source);

    let Dual {
        content,
        normalized,
    } = dual(&tree, tree.root_node());

    assert_ne!(content, 0);
    assert_ne!(normalized, 0);
}

#[test]
fn dual_content_differs_same() {
    let source1 = "fn foo() { let x = 1; }";
    let source2 = "fn bar() { let y = 1; }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let Dual {
        content: content1,
        normalized: normalized1,
    } = dual(&tree1, tree1.root_node());
    let Dual {
        content: content2,
        normalized: normalized2,
    } = dual(&tree2, tree2.root_node());

    assert_ne!(content1, content2);
    assert_eq!(normalized1, normalized2);
}

#[test]
fn javascript() {
    let source1 = "function add(a, b) { return a + b; }";
    let source2 = "function sum(x, y) { return x + y; }";

    let tree1 = parse_js(source1);
    let tree2 = parse_js(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_eq!(hash1, hash2);
}

#[test]
fn python() {
    let source1 = "def add(a, b):\n    return a + b";
    let source2 = "def sum(x, y):\n    return x + y";

    let tree1 = parse_python(source1);
    let tree2 = parse_python(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_eq!(hash1, hash2);
}

#[test]
fn empty_function() {
    let source = "fn empty() {}";
    let tree = parse_rust(source);

    let hash = normalized(&tree, tree.root_node());
    assert_ne!(hash, 0);
}

#[test]
fn nested_functions() {
    let source1 = r"
fn outer() {
    fn inner() {
        let x = 1;
    }
}
";
    let source2 = r"
fn wrapper() {
    fn nested() {
        let y = 1;
    }
}
";
    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_eq!(hash1, hash2);
}

#[test]
fn recursive_function() {
    let source1 = r"
fn factorial(n: i32) -> i32 {
    if n <= 1 { 1 } else { n * factorial(n - 1) }
}
";
    let source2 = r"
fn fact(x: i32) -> i32 {
    if x <= 1 { 1 } else { x * fact(x - 1) }
}
";
    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_eq!(hash1, hash2);
}

#[test]
fn closure() {
    let source1 = "fn f() { let add = |a, b| a + b; }";
    let source2 = "fn g() { let sum = |x, y| x + y; }";

    let tree1 = parse_rust(source1);
    let tree2 = parse_rust(source2);

    let hash1 = normalized(&tree1, tree1.root_node());
    let hash2 = normalized(&tree2, tree2.root_node());

    assert_eq!(hash1, hash2);
}

#[test]
fn deterministic() {
    let source = "fn foo() { let x = 1; let y = 2; x + y }";
    let tree = parse_rust(source);

    let hash1 = normalized(&tree, tree.root_node());
    let hash2 = normalized(&tree, tree.root_node());
    let hash3 = normalized(&tree, tree.root_node());

    assert_eq!(hash1, hash2);
    assert_eq!(hash2, hash3);
}
