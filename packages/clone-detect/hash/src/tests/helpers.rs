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
