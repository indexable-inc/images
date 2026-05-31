use ast_merge_ast::Tree;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::items::{IndexedNode, Resolved, get_name};

/// Context for collecting right-only items not present in base.
pub(crate) struct CollectNewContext<'a, 'tree> {
    pub(crate) right_items: &'a [tree_sitter::Node<'tree>],
    pub(crate) handled_right: &'a FxHashSet<usize>,
    pub(crate) base_by_name: &'a FxHashMap<String, IndexedNode<'tree>>,
    pub(crate) base_hashes: &'a FxHashMap<u64, usize>,
    pub(crate) right_to_base: &'a FxHashMap<usize, usize>,
}

pub(crate) fn collect_new(right_tree: &Tree, ctx: &CollectNewContext<'_, '_>) -> Vec<Resolved> {
    use ast_merge_ast::compute;

    let mut new_right_items: Vec<Resolved> = Vec::new();
    for (right_idx, right_item) in ctx.right_items.iter().enumerate() {
        if ctx.handled_right.contains(&right_idx) {
            continue;
        }

        let right_name = get_name(right_tree, *right_item);
        let right_hash = compute(right_tree, *right_item);
        let right_kind = right_item.kind().to_owned();

        let is_new = if let Some(name) = &right_name {
            !ctx.base_by_name.contains_key(name)
        } else {
            !ctx.base_hashes.contains_key(&right_hash)
        };

        if is_new {
            let predecessor_base_idx = (0..right_idx)
                .rev()
                .find_map(|prev_idx| ctx.right_to_base.get(&prev_idx).copied());

            new_right_items.push(Resolved {
                text: right_tree.node_text(*right_item).to_owned(),
                kind: right_kind,
                base_index: None,
                right_predecessor_base_idx: predecessor_base_idx,
            });
        }
    }
    new_right_items
}

pub(crate) fn insert_new(merged_items: &mut Vec<Resolved>, new_right_items: Vec<Resolved>) {
    for new_item in new_right_items {
        let insert_pos = if let Some(pred_base_idx) = new_item.right_predecessor_base_idx {
            merged_items
                .iter()
                .position(|m| m.base_index == Some(pred_base_idx))
                .map_or(merged_items.len(), |p| p + 1)
        } else {
            merged_items
                .iter()
                .position(|m| m.kind != new_item.kind)
                .unwrap_or(0)
        };

        merged_items.insert(insert_pos, new_item);
    }
}
