use ast_merge_ast::Tree;
use ast_merge_langs::Lang;

pub fn parse_rust(source: &str) -> Tree {
    let lang = Lang::Rust.to_tree_sitter();
    ast_merge_ast::tree(source, &lang).unwrap().tree
}

pub fn parse_js(source: &str) -> Tree {
    let lang = Lang::JavaScript.to_tree_sitter();
    ast_merge_ast::tree(source, &lang).unwrap().tree
}

pub fn parse_python(source: &str) -> Tree {
    let lang = Lang::Python.to_tree_sitter();
    ast_merge_ast::tree(source, &lang).unwrap().tree
}

pub fn pair_hashes(
    parse: fn(&str) -> Tree,
    hash: fn(&Tree, tree_sitter::Node<'_>) -> u64,
    left: &str,
    right: &str,
) -> (u64, u64) {
    let left = parse(left);
    let right = parse(right);
    (
        hash(&left, left.root_node()),
        hash(&right, right.root_node()),
    )
}
