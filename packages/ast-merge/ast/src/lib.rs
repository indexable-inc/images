mod hash;
mod parse;
mod types;

pub use hash::compute;
pub use parse::{Error, Output, PreorderIterator, SetLanguageSnafu, Tree, tree};
pub use types::{Node, NodeId, Revision};

#[cfg(test)]
mod tests {
    use crate::{Node, NodeId, Revision, compute, tree};

    fn get_rust_language() -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    #[test]
    fn test_parse_simple_rust() {
        let source = "fn main() {}";
        let result = tree(source, &get_rust_language()).unwrap();

        assert!(!result.has_errors);
        assert_eq!(result.tree.source(), source);
        assert_eq!(result.tree.root_node().kind(), "source_file");
    }

    #[test]
    fn test_parse_with_errors() {
        let source = "fn main( {}";
        let result = tree(source, &get_rust_language()).unwrap();

        assert!(result.has_errors);
    }

    #[test]
    fn test_node_text_extraction() {
        let source = "fn hello() { 42 }";
        let result = tree(source, &get_rust_language()).unwrap();

        let root = result.tree.root_node();
        assert_eq!(result.tree.node_text(root), source);
    }

    #[test]
    fn test_preorder_iteration() {
        let source = "fn a() {} fn b() {}";
        let result = tree(source, &get_rust_language()).unwrap();

        let nodes: Vec<_> = result.tree.preorder().collect();
        assert!(!nodes.is_empty());

        assert_eq!(
            nodes.first().map(tree_sitter::Node::kind),
            Some("source_file")
        );
    }

    #[test]
    fn test_content_hash_same_content() {
        let source1 = "fn foo() {}";
        let source2 = "fn foo() {}";

        let result1 = tree(source1, &get_rust_language()).unwrap();
        let result2 = tree(source2, &get_rust_language()).unwrap();

        let hash1 = compute(&result1.tree, result1.tree.root_node());
        let hash2 = compute(&result2.tree, result2.tree.root_node());

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_content_hash_different_content() {
        let source1 = "fn foo() {}";
        let source2 = "fn bar() {}";

        let result1 = tree(source1, &get_rust_language()).unwrap();
        let result2 = tree(source2, &get_rust_language()).unwrap();

        let hash1 = compute(&result1.tree, result1.tree.root_node());
        let hash2 = compute(&result2.tree, result2.tree.root_node());

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_revision_names() {
        assert_eq!(Revision::Base.name(), "BASE");
        assert_eq!(Revision::Left.name(), "LEFT");
        assert_eq!(Revision::Right.name(), "RIGHT");
    }

    #[test]
    fn test_node_creation() {
        let source = "let x = 1;";
        let result = tree(source, &get_rust_language()).unwrap();
        let root = result.tree.root_node();

        let ast_node = Node::new(root, NodeId(0));

        assert_eq!(ast_node.id(), NodeId(0));
        assert_eq!(ast_node.kind(), "source_file");
        assert!(ast_node.hash() != 0);
    }

    #[test]
    fn test_node_id_equality() {
        assert_eq!(NodeId(1), NodeId(1));
        assert_ne!(NodeId(1), NodeId(2));
    }
}
