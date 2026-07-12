mod integration;

use ast_merge_ast::tree;
use rustc_hash::FxHashSet;

use crate::{Map, compute, matching::Pair, traverse::node_height};

fn get_rust_language() -> tree_sitter::Language {
    tree_sitter_rust::LANGUAGE.into()
}

fn parse_rust(source: &str) -> ast_merge_ast::Tree {
    tree(source, &get_rust_language()).unwrap().tree
}

#[test]
fn structurally_related_trees_have_matches() {
    let cases = [
        ("identical", "fn foo() {}", "fn foo() {}"),
        ("renamed", "fn foo() {}", "fn bar() {}"),
        (
            "addition",
            "fn foo() {}",
            "fn foo() {} fn bar() {}",
        ),
        (
            "deletion",
            "fn foo() {} fn bar() {}",
            "fn foo() {}",
        ),
        (
            "reordering",
            "fn foo() {} fn bar() {}",
            "fn bar() {} fn foo() {}",
        ),
    ];

    for (name, source_a, source_b) in cases {
        let matching = compute(&parse_rust(source_a), &parse_rust(source_b));
        assert!(!matching.is_empty(), "{name}");
    }
}

#[test]
fn test_matching_empty_matching() {
    let matching = Map::new();
    assert!(matching.is_empty());
    assert_eq!(matching.len(), 0);
}

#[test]
fn test_matching_add_and_get() {
    let mut matching = Map::new();
    matching.add_match(0, 5);
    matching.add_match(1, 6);

    assert_eq!(matching.get_match_a_to_b(0), Some(5));
    assert_eq!(matching.get_match_a_to_b(1), Some(6));
    assert_eq!(matching.get_match_b_to_a(5), Some(0));
    assert_eq!(matching.get_match_b_to_a(6), Some(1));
    assert_eq!(matching.len(), 2);
}

#[test]
fn test_matching_is_matched() {
    let mut matching = Map::new();
    matching.add_match(0, 5);

    assert!(matching.is_matched_a(0));
    assert!(!matching.is_matched_a(1));
    assert!(matching.is_matched_b(5));
    assert!(!matching.is_matched_b(6));
}

#[test]
fn test_matching_iter() {
    let mut matching = Map::new();
    matching.add_match(0, 5);
    matching.add_match(1, 6);

    let pairs: FxHashSet<_> = matching.iter().collect();
    assert!(pairs.contains(&Pair { a_id: 0, b_id: 5 }));
    assert!(pairs.contains(&Pair { a_id: 1, b_id: 6 }));
}

#[test]
fn test_node_height() {
    let source = "fn foo() { let x = 1; }";
    let tree = parse_rust(source);
    let root = tree.root_node();

    let height = node_height(root);
    assert!(height > 1);
}

#[test]
fn test_config_default() {
    let config = crate::Config::default();
    assert_eq!(config.min_height, 2);
    assert!((config.dice_threshold - 0.5).abs() < f64::EPSILON);
    assert_eq!(config.max_ted_size, 100);
}
