mod left;
mod right;

use ast_merge_ast::Tree;
use left::{Context, NameLookups, reconcile_all};
use right::{CollectNewContext, collect_new, insert_new};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    children::{try_reconcile, try_reconcile_function},
    engine::{ThreeWayNodes, ThreeWayTrees},
};

#[derive(Debug)]
pub struct Resolved {
    pub text: String,
    pub kind: String,
    pub base_index: Option<usize>,
    pub right_predecessor_base_idx: Option<usize>,
}

/// A tree-sitter node paired with its positional index in a child list.
#[derive(Debug, Clone, Copy)]
pub struct IndexedNode<'tree> {
    pub index: usize,
    pub node: tree_sitter::Node<'tree>,
}

pub fn build_name_map<'a>(
    tree: &Tree,
    items: &[tree_sitter::Node<'a>],
) -> FxHashMap<String, IndexedNode<'a>> {
    items
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| {
            get_name(tree, *node).map(|name| {
                (
                    name,
                    IndexedNode {
                        index: idx,
                        node: *node,
                    },
                )
            })
        })
        .collect()
}

pub fn get_name(tree: &Tree, node: tree_sitter::Node<'_>) -> Option<String> {
    let kind = node.kind();

    let name_field = match kind {
        "function_item" | "struct_item" | "enum_item" | "trait_item" | "type_item"
        | "const_item" | "static_item" | "mod_item" | "field_declaration"
        | "function_signature_item" => Some("name"),
        "impl_item" => Some("type"),
        "use_declaration" => {
            return Some(tree.node_text(node).to_owned());
        }
        _ => None,
    }?;

    node.child_by_field_name(name_field)
        .map(|n| tree.node_text(n).to_owned())
}

pub fn reconcile_single(trees: &ThreeWayTrees<'_>, nodes: ThreeWayNodes<'_>) -> String {
    use ast_merge_ast::compute;

    let ThreeWayTrees {
        base: base_tree,
        left: left_tree,
        right: right_tree,
    } = trees;
    let ThreeWayNodes {
        base: base_node,
        left: left_node,
        right: right_node,
    } = nodes;

    let base_hash = compute(base_tree, base_node);
    let left_hash = compute(left_tree, left_node);
    let right_hash = compute(right_tree, right_node);

    if left_hash == right_hash {
        return left_tree.node_text(left_node).to_owned();
    }
    if left_hash == base_hash {
        return right_tree.node_text(right_node).to_owned();
    }
    if right_hash == base_hash {
        return left_tree.node_text(left_node).to_owned();
    }

    let kind = base_node.kind();
    if matches!(
        kind,
        "struct_item" | "impl_item" | "enum_item" | "trait_item"
    ) && let Some(merged) = try_reconcile(
        trees,
        ThreeWayNodes {
            base: base_node,
            left: left_node,
            right: right_node,
        },
    ) {
        return merged;
    }

    if kind == "function_item"
        && let Some(merged) = try_reconcile_function(
            trees,
            ThreeWayNodes {
                base: base_node,
                left: left_node,
                right: right_node,
            },
        )
    {
        return merged;
    }

    let left_text = left_tree.node_text(left_node);
    let right_text = right_tree.node_text(right_node);
    let base_text = base_tree.node_text(base_node);
    format!(
        "<<<<<<< LEFT\n{left_text}\n||||||| BASE\n{base_text}\n=======\n{right_text}\n>>>>>>> \
         RIGHT"
    )
}

/// Three-way item slices for `reconcile_lists`.
pub struct ThreeWay<'a, 'tree> {
    pub base: &'a [tree_sitter::Node<'tree>],
    pub left: &'a [tree_sitter::Node<'tree>],
    pub right: &'a [tree_sitter::Node<'tree>],
}

pub fn reconcile_lists(trees: &ThreeWayTrees<'_>, items: &ThreeWay<'_, '_>) -> String {
    let base_by_name = build_name_map(trees.base, items.base);
    let right_by_name = build_name_map(trees.right, items.right);
    let base_hashes = build_hash_map(trees.base, items.base);
    let right_to_base = build_right_to_base_map(
        trees.right,
        &RightToBaseInput {
            right_items: items.right,
            base_by_name: &base_by_name,
            base_hashes: &base_hashes,
        },
    );

    let mut handled_right: FxHashSet<usize> = FxHashSet::default();

    let name_lookups = NameLookups {
        base: &base_by_name,
        right: &right_by_name,
    };

    let left_ctx = Context {
        trees,
        left_items: items.left,
        right_items: items.right,
        name_lookups: &name_lookups,
        base_hashes: &base_hashes,
    };
    let mut merged_items = reconcile_all(&left_ctx, &mut handled_right);

    let collect_ctx = CollectNewContext {
        right_items: items.right,
        handled_right: &handled_right,
        base_by_name: &base_by_name,
        base_hashes: &base_hashes,
        right_to_base: &right_to_base,
    };
    let new_right_items = collect_new(trees.right, &collect_ctx);
    insert_new(&mut merged_items, new_right_items);

    let mut result = String::new();
    for item in merged_items {
        result.push_str(&item.text);
        result.push('\n');
    }
    result
}

fn build_hash_map(tree: &Tree, items: &[tree_sitter::Node<'_>]) -> FxHashMap<u64, usize> {
    use ast_merge_ast::compute;
    items
        .iter()
        .enumerate()
        .map(|(idx, node)| (compute(tree, *node), idx))
        .collect()
}

struct RightToBaseInput<'a, 'tree> {
    right_items: &'a [tree_sitter::Node<'tree>],
    base_by_name: &'a FxHashMap<String, IndexedNode<'tree>>,
    base_hashes: &'a FxHashMap<u64, usize>,
}

fn build_right_to_base_map(
    right_tree: &Tree,
    input: &RightToBaseInput<'_, '_>,
) -> FxHashMap<usize, usize> {
    use ast_merge_ast::compute;
    input
        .right_items
        .iter()
        .enumerate()
        .filter_map(|(right_idx, right_item)| {
            get_name(right_tree, *right_item).map_or_else(
                || {
                    let right_hash = compute(right_tree, *right_item);
                    input
                        .base_hashes
                        .get(&right_hash)
                        .map(|&base_idx| (right_idx, base_idx))
                },
                |name| {
                    input
                        .base_by_name
                        .get(&name)
                        .map(|entry| (right_idx, entry.index))
                },
            )
        })
        .collect()
}
