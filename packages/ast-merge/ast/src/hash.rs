use std::hash::{Hash, Hasher};

use rustc_hash::FxHasher;

use crate::Tree;

#[must_use]
pub fn compute(tree: &Tree, node: tree_sitter::Node<'_>) -> u64 {
    let mut hasher = FxHasher::default();
    compute_recursive(tree, node, &mut hasher);
    hasher.finish()
}

fn compute_recursive(tree: &Tree, node: tree_sitter::Node<'_>, hasher: &mut FxHasher) {
    node.kind().hash(hasher);

    if node.child_count() == 0 {
        tree.node_text(node).hash(hasher);
    } else {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            compute_recursive(tree, child, hasher);
        }
    }
}
