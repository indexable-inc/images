mod config;
mod gumtree;
mod matching;
mod metadata;
mod traverse;

use ast_merge_ast::Tree;
pub use config::Config;
pub use gumtree::GumTree;
pub use matching::{Map, Pair};

#[must_use]
pub fn compute(tree_a: &Tree, tree_b: &Tree) -> Map {
    let start = std::time::Instant::now();
    let matcher = GumTree::new(tree_a, tree_b, Config::default());
    let result = matcher.compute();
    tracing::debug!(
        elapsed_ms = start.elapsed().as_millis(),
        matches = result.len(),
        "compute complete"
    );
    result
}

#[cfg(test)]
mod tests;
