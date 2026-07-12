use super::helpers::{parse_python, parse_rust};
use crate::significant_nodes;

#[test]
fn extracts_significant_nodes() {
    let source = r"
fn small() {}

fn larger() {
    let x = 1;
    let y = 2;
    let z = x + y;
    z
}

struct Foo {
    a: i32,
    b: i32,
}
";

    let tree = parse_rust(source);
    let nodes = significant_nodes(&tree, 3, 5);

    assert!(!nodes.is_empty());
    for node in &nodes {
        assert!(node.end_line - node.start_line >= 2);
        assert!(node.node_count >= 5);
    }
}

#[test]
fn empty_file() {
    let source = "";
    let tree = parse_rust(source);
    let nodes = significant_nodes(&tree, 3, 5);
    assert!(nodes.is_empty());
}

#[test]
fn only_small_functions() {
    let source = "fn a() {} fn b() {} fn c() {}";
    let tree = parse_rust(source);
    let nodes = significant_nodes(&tree, 5, 10);

    assert!(nodes.is_empty());
}

#[test]
fn significant_nodes_cover_rust_impls_and_python_classes() {
    let cases = [
        (
            parse_rust as fn(&str) -> ast_merge_ast::Tree,
            r"
impl Foo {
    fn method1(&self) {
        let x = 1;
        let y = 2;
        x + y
    }

    fn method2(&self) {
        let a = 3;
        let b = 4;
        a * b
    }
}
",
        ),
        (
            parse_python as fn(&str) -> ast_merge_ast::Tree,
            r"
def small():
    pass

def larger():
    x = 1
    y = 2
    z = x + y
    return z

class Foo:
    def __init__(self):
        self.a = 1
        self.b = 2
",
        ),
    ];

    for (parse, source) in cases {
        let tree = parse(source);
        assert!(!significant_nodes(&tree, 3, 5).is_empty());
    }
}

#[test]
fn node_info_fields() {
    let source = r"
fn test_function() {
    let x = 1;
    let y = 2;
    let z = x + y;
    z
}
";
    let tree = parse_rust(source);
    let nodes = significant_nodes(&tree, 3, 5);

    assert!(!nodes.is_empty());
    let node = nodes.first().unwrap();

    assert!(node.content_hash != 0);
    assert!(node.normalized_hash != 0);
    assert!(!node.kind.is_empty());
    assert!(node.byte_range.start < node.byte_range.end);
    assert!(node.start_line <= node.end_line);
    assert!(node.node_count > 0);
}
