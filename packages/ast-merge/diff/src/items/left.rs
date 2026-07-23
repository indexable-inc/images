use ast_merge_ast::Tree;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    engine::{ThreeWayNodes, ThreeWayTrees},
    items::{IndexedNode, Resolved, get_name, reconcile_single},
};

/// Name-map lookups for base and right items.
pub struct NameLookups<'a, 'tree> {
    pub base: &'a FxHashMap<String, IndexedNode<'tree>>,
    pub right: &'a FxHashMap<String, IndexedNode<'tree>>,
}

/// Context for merging left items against base and right.
pub struct Context<'a, 'tree> {
    pub trees: &'a ThreeWayTrees<'tree>,
    pub left_items: &'a [tree_sitter::Node<'tree>],
    pub right_items: &'a [tree_sitter::Node<'tree>],
    pub name_lookups: &'a NameLookups<'a, 'tree>,
    pub base_hashes: &'a FxHashMap<u64, usize>,
}

pub fn reconcile_all(ctx: &Context<'_, '_>, handled_right: &mut FxHashSet<usize>) -> Vec<Resolved> {
    use ast_merge_ast::compute;

    let left_tree = ctx.trees.left;
    let right_tree = ctx.trees.right;
    let mut merged_items: Vec<Resolved> = Vec::new();

    for left_item in ctx.left_items {
        let left_hash = compute(left_tree, *left_item);
        let left_text = left_tree.node_text(*left_item);
        let left_name = get_name(left_tree, *left_item);
        let left_kind = left_item.kind().to_owned();

        if let Some(name) = &left_name {
            let input = NamedInput {
                trees: ctx.trees,
                name,
                left_item: *left_item,
                left_text,
                left_kind: &left_kind,
                lookups: ctx.name_lookups,
            };
            reconcile_named(&input, handled_right, &mut merged_items);
        } else {
            let input = UnnamedInput {
                left_hash,
                left_text,
                left_kind,
                left_item: *left_item,
                right_tree,
                right_items: ctx.right_items,
                base_hashes: ctx.base_hashes,
            };
            reconcile_unnamed(&input, handled_right, &mut merged_items);
        }
    }

    merged_items
}

struct NamedInput<'a, 'tree> {
    trees: &'a ThreeWayTrees<'tree>,
    name: &'a str,
    left_item: tree_sitter::Node<'tree>,
    left_text: &'a str,
    left_kind: &'a str,
    lookups: &'a NameLookups<'a, 'tree>,
}

fn reconcile_named(
    input: &NamedInput<'_, '_>,
    handled_right: &mut FxHashSet<usize>,
    merged_items: &mut Vec<Resolved>,
) {
    let base_match = input.lookups.base.get(input.name);
    let right_match = input.lookups.right.get(input.name);

    match (base_match, right_match) {
        (Some(base_entry), Some(right_entry)) => {
            handled_right.insert(right_entry.index);
            let merged = reconcile_single(
                input.trees,
                ThreeWayNodes {
                    base: base_entry.node,
                    left: input.left_item,
                    right: right_entry.node,
                },
            );
            merged_items.push(Resolved {
                text: merged,
                kind: input.left_kind.to_owned(),
                base_index: Some(base_entry.index),
                right_predecessor_base_idx: None,
            });
        }
        (Some(base_entry), None) => {
            merged_items.push(Resolved {
                text: input.left_text.to_owned(),
                kind: input.left_kind.to_owned(),
                base_index: Some(base_entry.index),
                right_predecessor_base_idx: None,
            });
        }
        (None, Some(right_entry)) => {
            handled_right.insert(right_entry.index);
            merged_items.push(Resolved {
                text: input.left_text.to_owned(),
                kind: input.left_kind.to_owned(),
                base_index: None,
                right_predecessor_base_idx: None,
            });
        }
        (None, None) => {
            merged_items.push(Resolved {
                text: input.left_text.to_owned(),
                kind: input.left_kind.to_owned(),
                base_index: None,
                right_predecessor_base_idx: None,
            });
        }
    }
}

struct UnnamedInput<'a, 'tree> {
    left_hash: u64,
    left_text: &'a str,
    left_kind: String,
    left_item: tree_sitter::Node<'tree>,
    right_tree: &'a Tree,
    right_items: &'a [tree_sitter::Node<'tree>],
    base_hashes: &'a FxHashMap<u64, usize>,
}

fn reconcile_unnamed(
    input: &UnnamedInput<'_, '_>,
    handled_right: &mut FxHashSet<usize>,
    merged_items: &mut Vec<Resolved>,
) {
    use ast_merge_ast::compute;

    let right_match = input.right_items.iter().enumerate().find(|(idx, r)| {
        !handled_right.contains(idx) && compute(input.right_tree, **r) == input.left_hash
    });

    if let Some((right_idx, _)) = right_match {
        handled_right.insert(right_idx);
        let base_idx = input.base_hashes.get(&input.left_hash).copied();
        merged_items.push(Resolved {
            text: input.left_text.to_owned(),
            kind: input.left_kind.clone(),
            base_index: base_idx,
            right_predecessor_base_idx: None,
        });
        return;
    }

    let Some(&base_idx) = input.base_hashes.get(&input.left_hash) else {
        merged_items.push(Resolved {
            text: input.left_text.to_owned(),
            kind: input.left_kind.clone(),
            base_index: None,
            right_predecessor_base_idx: None,
        });
        return;
    };

    let right_candidate = input.right_items.iter().enumerate().find(|(idx, r)| {
        !handled_right.contains(idx)
            && r.kind() == input.left_item.kind()
            && get_name(input.right_tree, **r).is_none()
    });

    let Some((right_idx, right_item)) = right_candidate else {
        merged_items.push(Resolved {
            text: input.left_text.to_owned(),
            kind: input.left_kind.clone(),
            base_index: Some(base_idx),
            right_predecessor_base_idx: None,
        });
        return;
    };

    let right_hash = compute(input.right_tree, *right_item);
    if input.base_hashes.contains_key(&right_hash) {
        merged_items.push(Resolved {
            text: input.left_text.to_owned(),
            kind: input.left_kind.clone(),
            base_index: Some(base_idx),
            right_predecessor_base_idx: None,
        });
        return;
    }

    handled_right.insert(right_idx);
    merged_items.push(Resolved {
        text: input.right_tree.node_text(*right_item).to_owned(),
        kind: input.left_kind.clone(),
        base_index: Some(base_idx),
        right_predecessor_base_idx: None,
    });
}
