use std::hash::{Hash, Hasher};

use ast_merge_ast::Tree;
pub use ast_merge_ast::compute;
use rustc_hash::FxHasher;

use crate::{
    kinds::{is_identifier, is_normalizable, is_significant},
    normalize,
};

/// Placeholder token for identifiers in subtree feature extraction.
const TOKEN_IDENT: u64 = 0x1234_5678_AAAA_BBBB;
/// Placeholder token for literals in subtree feature extraction.
const TOKEN_LITERAL: u64 = 0x9876_5432_CCCC_DDDD;

/// Position and hash info for a direct named child of a significant node.
/// Used for statement-sequence clone detection.
#[derive(Debug, Clone)]
pub struct ChildInfo {
    pub normalized_hash: u64,
    pub byte_range: std::ops::Range<usize>,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub content_hash: u64,
    pub normalized_hash: u64,
    pub kind: &'static str,
    pub byte_range: std::ops::Range<usize>,
    pub start_line: usize,
    pub end_line: usize,
    pub node_count: usize,
    /// Direct named children with position info. Ordered. Used for
    /// statement-sequence clone detection (sliding-window k-grams).
    pub children: Vec<ChildInfo>,
    /// Normalized structural tokens (unigrams + bigrams) for the entire
    /// subtree. Used as the feature set for MinHash LSH Type-3 detection.
    /// Much richer than direct-child hashes: captures deep structural
    /// similarity including nested patterns.
    pub subtree_features: Vec<u64>,
}

#[derive(Debug, Clone, Copy)]
pub struct Dual {
    pub content: u64,
    pub normalized: u64,
}

#[must_use]
pub fn dual(tree: &Tree, node: tree_sitter::Node<'_>) -> Dual {
    Dual {
        content: compute(tree, node),
        normalized: normalize::hash(tree, node),
    }
}

#[must_use]
pub fn significant_nodes(tree: &Tree, min_lines: usize, min_nodes: usize) -> Vec<NodeInfo> {
    let mut results = Vec::new();

    for node in tree.preorder() {
        let kind: &'static str = node.kind();
        if !is_significant(kind) {
            continue;
        }

        let start_line = node.start_position().row;
        let end_line = node.end_position().row;
        let line_count = end_line.saturating_sub(start_line) + 1;

        if line_count < min_lines {
            continue;
        }

        let node_count = count_nodes(node);
        if node_count < min_nodes {
            continue;
        }

        let Dual {
            content: content_hash,
            normalized: normalized_hash,
        } = dual(tree, node);

        let children = collect_children(tree, node);
        let subtree_features = collect_subtree_features(tree, node);

        results.push(NodeInfo {
            content_hash,
            normalized_hash,
            kind,
            byte_range: node.byte_range(),
            start_line,
            end_line,
            node_count,
            children,
            subtree_features,
        });
    }

    results
}

/// Collect detailed info for each direct named child of a node.
/// Preserves ordering for statement-sequence detection.
fn collect_children(tree: &Tree, node: tree_sitter::Node<'_>) -> Vec<ChildInfo> {
    let mut children = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        children.push(ChildInfo {
            normalized_hash: normalize::hash(tree, child),
            byte_range: child.byte_range(),
            start_line: child.start_position().row,
            end_line: child.end_position().row,
        });
    }
    children
}

/// Collect structural features for MinHash LSH.
///
/// Walks the subtree in preorder, producing a normalized token per AST node
/// (kind + normalized leaf content). Then adds bigrams of consecutive tokens
/// for structural context. The combined unigram+bigram set captures both
/// local node types and parent-child/sibling relationships.
///
/// Complexity: O(n) where n = number of nodes in subtree.
fn collect_subtree_features(tree: &Tree, node: tree_sitter::Node<'_>) -> Vec<u64> {
    let mut tokens = Vec::new();
    collect_tokens_recursive(tree, node, &mut tokens);

    let bigram_count = tokens.len().saturating_sub(1);
    let mut features = Vec::with_capacity(tokens.len() + bigram_count);
    features.extend_from_slice(&tokens);

    // Add bigrams for structural context (parent-child, sibling relationships)
    for window in tokens.windows(2) {
        let mut hasher = FxHasher::default();
        // windows(2) guarantees exactly 2 elements per window
        if let [a, b] = *window {
            a.hash(&mut hasher);
            b.hash(&mut hasher);
        }
        features.push(hasher.finish());
    }

    features.sort_unstable();
    features
}

fn collect_tokens_recursive(tree: &Tree, node: tree_sitter::Node<'_>, tokens: &mut Vec<u64>) {
    let mut hasher = FxHasher::default();
    let kind = node.kind();
    kind.hash(&mut hasher);

    if node.child_count() == 0 {
        if is_normalizable(kind) {
            if is_identifier(kind) {
                TOKEN_IDENT.hash(&mut hasher);
            } else {
                TOKEN_LITERAL.hash(&mut hasher);
            }
        } else {
            tree.node_text(node).hash(&mut hasher);
        }
    }

    tokens.push(hasher.finish());

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_tokens_recursive(tree, child, tokens);
    }
}

fn count_nodes(node: tree_sitter::Node<'_>) -> usize {
    let mut count = 1;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        count += count_nodes(child);
    }
    count
}
