use ast_merge_ast::Tree;
use ast_merge_langs::Lang;

pub fn parse(lang: Lang, source: &str) -> Tree {
    ast_merge_ast::tree(source, &lang.to_tree_sitter())
        .unwrap()
        .tree
}

pub fn parse_rust(source: &str) -> Tree {
    parse(Lang::Rust, source)
}

pub fn parse_js(source: &str) -> Tree {
    parse(Lang::JavaScript, source)
}

pub struct HashPair {
    pub left: u64,
    pub right: u64,
}

pub fn pair_hashes(
    parse: fn(&str) -> Tree,
    hash: fn(&Tree, tree_sitter::Node<'_>) -> u64,
    left: &str,
    right: &str,
) -> HashPair {
    let left = parse(left);
    let right = parse(right);
    HashPair {
        left: hash(&left, left.root_node()),
        right: hash(&right, right.root_node()),
    }
}
