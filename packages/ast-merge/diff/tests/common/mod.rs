#![expect(clippy::expect_used, reason = "Test helpers - expect is acceptable")]

use ast_merge_ast::Tree;

pub fn parse_rust(source: &str) -> Tree {
    let lang = ast_merge_langs::Lang::Rust.to_tree_sitter();
    ast_merge_ast::tree(source, &lang).expect("valid rust").tree
}

pub fn rust(base: &str, left: &str, right: &str) -> ast_merge_diff::Result {
    let base_tree = parse_rust(base);
    let left_tree = parse_rust(left);
    let right_tree = parse_rust(right);

    let base_left = ast_merge_matcher::compute(&base_tree, &left_tree);
    let base_right = ast_merge_matcher::compute(&base_tree, &right_tree);

    let merger = ast_merge_diff::ThreeWay::new(ast_merge_diff::ThreeWayParams {
        trees: ast_merge_diff::ThreeWayTrees {
            base: &base_tree,
            left: &left_tree,
            right: &right_tree,
        },
        base_left_matching: base_left,
        base_right_matching: base_right,
        config: ast_merge_diff::Config::default(),
    });

    merger.merge()
}
