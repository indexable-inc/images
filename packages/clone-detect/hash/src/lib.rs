mod extract;
pub mod kinds;
mod normalize;

pub use ast_merge_ast::compute;
pub use extract::{ChildInfo, Dual, NodeInfo, dual, significant_nodes};
pub use kinds::is_significant;
pub use normalize::hash;

#[cfg(test)]
mod tests {
    mod content;
    mod extract;
    mod helpers;
    mod kinds;
    mod normalized;
}
