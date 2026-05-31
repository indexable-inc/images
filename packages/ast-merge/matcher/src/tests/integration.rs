use ast_merge_ast::tree;

use crate::{compute, metadata::compute_node_meta, traverse::assign_node_ids};

fn get_rust_language() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}

fn parse_rust(source: &str) -> ast_merge_ast::Tree {
    tree(source, &get_rust_language()).unwrap().tree
}

fn get_toml_language() -> tree_sitter::Language {
    tree_sitter_toml_ng::LANGUAGE.into()
}

fn parse_toml(source: &str) -> ast_merge_ast::Tree {
    tree(source, &get_toml_language()).unwrap().tree
}

#[test]
fn test_matching_identical_comments_with_insertion() {
    let source_a = r#"# Comment 1
key1 = "value1"
# Comment 2
key2 = "value2"
"#;
    let source_b = r#"# New comment
key0 = "value0"
# Comment 1
key1 = "value1"
# Comment 2
key2 = "value2"
"#;

    let tree_a = parse_toml(source_a);
    let tree_b = parse_toml(source_b);

    let matching = compute(&tree_a, &tree_b);

    let mut nodes_a = Vec::new();
    let mut nodes_b = Vec::new();
    assign_node_ids(tree_a.root_node(), &mut nodes_a);
    assign_node_ids(tree_b.root_node(), &mut nodes_b);

    let comment_matches: Vec<_> = matching
        .iter()
        .filter(|pair| {
            nodes_a
                .get(pair.a_id)
                .is_some_and(|node| node.kind() == "comment")
        })
        .collect();

    assert!(
        comment_matches.len() >= 2,
        "Expected at least 2 comment matches, got {}",
        comment_matches.len()
    );
}

#[test]
fn test_matching_identical_siblings_same_parent() {
    let source_a = r#"# First
# Second
key = "value"
"#;
    let source_b = r#"# First
# Second
key = "value"
"#;

    let tree_a = parse_toml(source_a);
    let tree_b = parse_toml(source_b);

    let matching = compute(&tree_a, &tree_b);

    let mut nodes_a = Vec::new();
    assign_node_ids(tree_a.root_node(), &mut nodes_a);

    for (a_id, node) in nodes_a.iter().enumerate() {
        assert!(
            matching.is_matched_a(a_id),
            "Node {} ({}) not matched",
            a_id,
            node.kind()
        );
    }
}

#[test]
fn test_function_rename_with_similar_body() {
    let source_a = r#"
fn build_line<'a>(
    row: &DiffRow,
    theme: &'a Theme,
) -> Line<'a> {
    let mut spans = Vec::new();
    spans.push(Span::styled("test", Style::default()));
    Line::from(spans)
}
"#;
    let source_b = r#"
/// Build a content line for a row.
fn build_content_line<'a>(
    row: &ContentRow,
    theme: &'a Theme,
) -> Line<'a> {
    let mut spans = Vec::new();
    spans.push(Span::styled("test", Style::default()));
    Line::from(spans)
}
"#;

    let tree_a = parse_rust(source_a);
    let tree_b = parse_rust(source_b);

    let matching = compute(&tree_a, &tree_b);

    let mut nodes_a = Vec::new();
    let mut nodes_b = Vec::new();
    assign_node_ids(tree_a.root_node(), &mut nodes_a);
    assign_node_ids(tree_b.root_node(), &mut nodes_b);

    let fn_a = nodes_a
        .iter()
        .position(|n| n.kind() == "function_item")
        .expect("should have function_item in A");
    let fn_b = nodes_b
        .iter()
        .position(|n| n.kind() == "function_item")
        .expect("should have function_item in B");

    assert!(
        matching.get_match_a_to_b(fn_a) == Some(fn_b),
        "Expected build_line to match build_content_line. fn_a={fn_a} matched to {:?}, fn_b={fn_b}",
        matching.get_match_a_to_b(fn_a)
    );
}

fn count_descendants_recursive(node: tree_sitter::Node<'_>) -> usize {
    let mut count = 1;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        count += count_descendants_recursive(child);
    }
    count
}

#[test]
fn test_descendant_count_matches_recursive() {
    let source = r#"
fn foo() {
    let x = 1;
    if x > 0 {
        println!("positive");
    } else {
        println!("non-positive");
    }
}

fn bar(a: i32, b: i32) -> i32 {
    a + b
}

struct Point {
    x: f64,
    y: f64,
}

impl Point {
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}
"#;
    let tree = parse_rust(source);
    let mut nodes = Vec::new();
    assign_node_ids(tree.root_node(), &mut nodes);

    let meta = compute_node_meta(&nodes);

    for (i, node) in nodes.iter().enumerate() {
        let expected = count_descendants_recursive(*node);
        let actual = meta[i].descendants;
        assert_eq!(
            actual,
            expected,
            "Node {} ({:?}) at {:?}: expected {} descendants, got {}",
            i,
            node.kind(),
            node.byte_range(),
            expected,
            actual
        );
    }
}

#[test]
fn test_descendant_count_simple() {
    let source = "fn foo() {}";
    let tree = parse_rust(source);
    let mut nodes = Vec::new();
    assign_node_ids(tree.root_node(), &mut nodes);

    let meta = compute_node_meta(&nodes);

    assert_eq!(meta[0].descendants, nodes.len());

    for (i, node) in nodes.iter().enumerate() {
        if node.child_count() == 0 {
            assert_eq!(
                meta[i].descendants,
                1,
                "Leaf node {} ({:?}) should have 1 descendant",
                i,
                node.kind()
            );
        }
    }
}
