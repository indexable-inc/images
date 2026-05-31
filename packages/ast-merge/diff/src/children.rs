mod body;

use ast_merge_ast::Tree;
pub use body::try_reconcile_function;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    engine::{ThreeWayNodes, ThreeWayTrees},
    items::{IndexedNode, build_name_map, get_name, reconcile_single},
};

pub fn try_reconcile(trees: &ThreeWayTrees<'_>, nodes: ThreeWayNodes<'_>) -> Option<String> {
    let ThreeWayNodes {
        base: base_node,
        left: left_node,
        right: right_node,
    } = nodes;

    let body_field = match base_node.kind() {
        "struct_item" | "impl_item" | "enum_item" | "trait_item" => "body",
        _ => return None,
    };

    let base_body = base_node.child_by_field_name(body_field)?;
    let left_body = left_node.child_by_field_name(body_field)?;
    let right_body = right_node.child_by_field_name(body_field)?;

    let base_children: Vec<_> = get_meaningful(base_body);
    let left_children: Vec<_> = get_meaningful(left_body);
    let right_children: Vec<_> = get_meaningful(right_body);

    let children = ThreeWay {
        base: &base_children,
        left: &left_children,
        right: &right_children,
    };
    let merged_children = reconcile_lists(trees, &children);

    let reconstruct = ReconstructBodyInput {
        left_node,
        left_body,
        base_node,
        merged_children: &merged_children,
    };
    Some(reconstruct_body(trees.left, &reconstruct))
}

struct ThreeWay<'a, 'tree> {
    base: &'a [tree_sitter::Node<'tree>],
    left: &'a [tree_sitter::Node<'tree>],
    right: &'a [tree_sitter::Node<'tree>],
}

fn reconcile_lists(trees: &ThreeWayTrees<'_>, children: &ThreeWay<'_, '_>) -> Vec<String> {
    use ast_merge_ast::compute;

    let base_child_names = build_name_map(trees.base, children.base);
    let right_child_names = build_name_map(trees.right, children.right);

    let base_child_hashes: FxHashMap<u64, usize> = children
        .base
        .iter()
        .enumerate()
        .map(|(idx, node)| (compute(trees.base, *node), idx))
        .collect();

    let mut merged_children = Vec::new();
    let mut handled_right: FxHashSet<usize> = FxHashSet::default();

    let lookups = ChildNameLookups {
        base: &base_child_names,
        right: &right_child_names,
    };

    for left_child in children.left {
        let left_name = get_name(trees.left, *left_child);
        let left_hash = compute(trees.left, *left_child);

        if let Some(name) = &left_name {
            reconcile_named(
                &NamedChildInput {
                    trees,
                    name,
                    left_child: *left_child,
                    left_hash,
                    lookups: &lookups,
                },
                &mut handled_right,
                &mut merged_children,
            );
        } else {
            reconcile_unnamed(
                &UnnamedChildInput {
                    left_hash,
                    left_child: *left_child,
                    left_tree: trees.left,
                    right_tree: trees.right,
                    right_children: children.right,
                },
                &mut handled_right,
                &mut merged_children,
            );
        }
    }

    let collect = CollectNewRightInput {
        right_children: children.right,
        handled_right: &handled_right,
        base_child_names: &base_child_names,
        base_child_hashes: &base_child_hashes,
    };
    collect_new_right(trees.right, &collect, &mut merged_children);

    merged_children
}

struct ChildNameLookups<'a, 'tree> {
    base: &'a FxHashMap<String, IndexedNode<'tree>>,
    right: &'a FxHashMap<String, IndexedNode<'tree>>,
}

struct NamedChildInput<'a, 'tree> {
    trees: &'a ThreeWayTrees<'tree>,
    name: &'a str,
    left_child: tree_sitter::Node<'tree>,
    left_hash: u64,
    lookups: &'a ChildNameLookups<'a, 'tree>,
}

fn reconcile_named(
    input: &NamedChildInput<'_, '_>,
    handled_right: &mut FxHashSet<usize>,
    merged_children: &mut Vec<String>,
) {
    use ast_merge_ast::compute;

    let base_match = input.lookups.base.get(input.name);
    let right_match = input.lookups.right.get(input.name);

    if let (Some(base_entry), Some(right_entry)) = (base_match, right_match) {
        handled_right.insert(right_entry.index);
        let base_h = compute(input.trees.base, base_entry.node);
        let right_h = compute(input.trees.right, right_entry.node);

        if input.left_hash == right_h {
            merged_children.push(input.trees.left.node_text(input.left_child).to_owned());
        } else if input.left_hash == base_h {
            merged_children.push(input.trees.right.node_text(right_entry.node).to_owned());
        } else if right_h == base_h {
            merged_children.push(input.trees.left.node_text(input.left_child).to_owned());
        } else {
            let merged = reconcile_single(
                input.trees,
                ThreeWayNodes {
                    base: base_entry.node,
                    left: input.left_child,
                    right: right_entry.node,
                },
            );
            merged_children.push(merged);
        }
    } else {
        if let Some(right_entry) = right_match {
            handled_right.insert(right_entry.index);
        }
        merged_children.push(input.trees.left.node_text(input.left_child).to_owned());
    }
}

struct UnnamedChildInput<'a, 'tree> {
    left_hash: u64,
    left_child: tree_sitter::Node<'tree>,
    left_tree: &'a Tree,
    right_tree: &'a Tree,
    right_children: &'a [tree_sitter::Node<'tree>],
}

fn reconcile_unnamed(
    input: &UnnamedChildInput<'_, '_>,
    handled_right: &mut FxHashSet<usize>,
    merged_children: &mut Vec<String>,
) {
    use ast_merge_ast::compute;

    let right_match = input.right_children.iter().enumerate().find(|(idx, r)| {
        !handled_right.contains(idx) && compute(input.right_tree, **r) == input.left_hash
    });

    if let Some((right_idx, _)) = right_match {
        handled_right.insert(right_idx);
    }
    merged_children.push(input.left_tree.node_text(input.left_child).to_owned());
}

struct CollectNewRightInput<'a, 'tree> {
    right_children: &'a [tree_sitter::Node<'tree>],
    handled_right: &'a FxHashSet<usize>,
    base_child_names: &'a FxHashMap<String, IndexedNode<'tree>>,
    base_child_hashes: &'a FxHashMap<u64, usize>,
}

fn collect_new_right(
    right_tree: &Tree,
    input: &CollectNewRightInput<'_, '_>,
    merged_children: &mut Vec<String>,
) {
    use ast_merge_ast::compute;

    for (idx, right_child) in input.right_children.iter().enumerate() {
        if input.handled_right.contains(&idx) {
            continue;
        }

        let right_name = get_name(right_tree, *right_child);
        let right_hash = compute(right_tree, *right_child);

        let is_new = right_name.as_ref().map_or_else(
            || !input.base_child_hashes.contains_key(&right_hash),
            |name| !input.base_child_names.contains_key(name),
        );

        if is_new {
            merged_children.push(right_tree.node_text(*right_child).to_owned());
        }
    }
}

struct ReconstructBodyInput<'a, 'tree> {
    left_node: tree_sitter::Node<'tree>,
    left_body: tree_sitter::Node<'tree>,
    base_node: tree_sitter::Node<'tree>,
    merged_children: &'a [String],
}

#[expect(
    clippy::string_slice,
    reason = "byte offsets from tree-sitter are guaranteed to be at UTF-8 char boundaries"
)]
fn reconstruct_body(left_tree: &Tree, input: &ReconstructBodyInput<'_, '_>) -> String {
    let left_text = left_tree.node_text(input.left_node);
    let left_body_text = left_tree.node_text(input.left_body);

    let body_start = input.left_body.start_byte() - input.left_node.start_byte();
    let body_end = input.left_body.end_byte() - input.left_node.start_byte();

    let prefix = &left_text[..body_start];
    let suffix = &left_text[body_end..];

    let (open, close) = if left_body_text.as_bytes().first() == Some(&b'{') {
        ("{", "}")
    } else {
        ("(", ")")
    };

    let mut result = String::new();
    result.push_str(prefix);
    result.push_str(open);
    result.push('\n');
    let needs_commas = matches!(input.base_node.kind(), "struct_item" | "enum_item");
    for child in input.merged_children {
        result.push_str("    ");
        result.push_str(child.trim());
        if needs_commas && !child.trim().ends_with(',') {
            result.push(',');
        }
        result.push('\n');
    }
    result.push_str(close);
    result.push_str(suffix);
    result
}

pub fn get_meaningful(node: tree_sitter::Node<'_>) -> Vec<tree_sitter::Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|n| {
            !n.is_extra()
                && !matches!(
                    n.kind(),
                    "{" | "}" | "(" | ")" | "[" | "]" | "," | ";" | "<" | ">"
                )
        })
        .collect()
}
