//! Nix language support for the [tree-sitter][] parsing library, forked from
//! [nix-community/tree-sitter-nix] 0.3.0 to accept underscore digit
//! separators in numeric literals (`1_000`, `1_000.000_1`, `2.5e1_0`) --
//! the dialect the repo's patched nix (`nix-ix`) parses. See this crate's
//! README for the fork rationale and regeneration instructions.
//!
//! [tree-sitter]: https://tree-sitter.github.io/
//! [nix-community/tree-sitter-nix]: https://github.com/nix-community/tree-sitter-nix

use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_nix() -> *const ();
}

/// The tree-sitter [`LanguageFn`] for this grammar.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_nix) };

/// The content of the [`node-types.json`][] file for this grammar.
///
/// [`node-types.json`]: https://tree-sitter.github.io/tree-sitter/using-parsers#static-node-types
pub const NODE_TYPES: &str = include_str!("node-types.json");

/// The syntax highlighting query for this language.
pub const HIGHLIGHTS_QUERY: &str = include_str!("queries/highlights.scm");

/// The injections query for this language.
pub const INJECTIONS_QUERY: &str = include_str!("queries/injections.scm");

#[cfg(test)]
mod tests {
    /// Kinds of the expression nodes bound in `{ a = ...; b = ...; }`
    /// source, in document order.
    fn binding_kinds(source: &str) -> Vec<String> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("Error loading nix parser");
        let tree = parser.parse(source, None).expect("parse returned no tree");
        assert!(!tree.root_node().has_error(), "parse error in {source:?}");
        let mut kinds = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            if node.kind() == "binding" {
                let expression = node
                    .child_by_field_name("expression")
                    .expect("binding without expression");
                kinds.push(expression.kind().to_owned());
                continue;
            }
            let mut cursor = node.walk();
            let mut children: Vec<_> = node.children(&mut cursor).collect();
            children.reverse();
            stack.extend(children);
        }
        kinds
    }

    /// The fork's delta: underscore digit separators lex as part of one
    /// number token instead of splitting into an application chain, a
    /// leading underscore still starts an identifier, and upstream lexing
    /// is unchanged for separator-free literals.
    #[test]
    fn parses_underscore_literals_as_numbers() {
        assert_eq!(
            binding_kinds("{ a = 1_000; b = 1_000.000_1; c = 2.5e1_0; d = _1_000; }"),
            [
                "integer_expression",
                "float_expression",
                "float_expression",
                "variable_expression",
            ]
        );
        assert_eq!(
            binding_kinds("{ a = 10000; b = 1000.5; c = .27e13; d = 1.; }"),
            [
                "integer_expression",
                "float_expression",
                "float_expression",
                "float_expression",
            ]
        );
    }
}
