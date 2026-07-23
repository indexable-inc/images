use rustc_hash::FxHashMap;

use crate::{
    matching::Map,
    traverse::{SiblingNodes, SubtreesInput, subtrees},
};

#[derive(Clone)]
struct Entry {
    node_id: usize,
    child_index: usize,
}

const ROOT_SENTINEL: usize = usize::MAX;
const ROOT_CHILDREN_SENTINEL: usize = usize::MAX - 1;

struct NodeClassification {
    parent_key: usize,
    child_index: usize,
}

fn classify_node(
    node: tree_sitter::Node<'_>,
    root_id: usize,
    nodes: &[tree_sitter::Node<'_>],
) -> NodeClassification {
    let Some(parent) = node.parent() else {
        return NodeClassification {
            parent_key: ROOT_SENTINEL,
            child_index: 0,
        };
    };

    let child_idx = parent
        .children(&mut parent.walk())
        .position(|c| c.id() == node.id())
        .unwrap_or(0);

    if parent.id() == root_id {
        NodeClassification {
            parent_key: ROOT_CHILDREN_SENTINEL,
            child_index: child_idx,
        }
    } else {
        let parent_id = nodes
            .iter()
            .position(|n| n.id() == parent.id())
            .unwrap_or(ROOT_SENTINEL);
        NodeClassification {
            parent_key: parent_id,
            child_index: child_idx,
        }
    }
}

struct ParentIndexInput<'a, 'b, F> {
    ids: &'b [usize],
    nodes: &'b [tree_sitter::Node<'a>],
    root_id: usize,
    matching_filter: F,
}

fn build_parent_index(
    input: &ParentIndexInput<'_, '_, impl Fn(usize) -> bool>,
) -> FxHashMap<usize, Vec<Entry>> {
    let mut by_parent: FxHashMap<usize, Vec<Entry>> = FxHashMap::default();

    for &id in input.ids {
        if (input.matching_filter)(id) {
            continue;
        }

        let Some(node) = input.nodes.get(id).copied() else {
            debug_assert!(
                false,
                "id {id} came from ids which must be valid node indices"
            );
            continue;
        };

        let classification = classify_node(node, input.root_id, input.nodes);
        by_parent
            .entry(classification.parent_key)
            .or_default()
            .push(Entry {
                node_id: id,
                child_index: classification.child_index,
            });
    }

    by_parent
}

pub struct Input<'a, 'b> {
    pub a: SiblingNodes<'a, 'b>,
    pub b: SiblingNodes<'a, 'b>,
    pub root_a: tree_sitter::Node<'a>,
    pub root_b: tree_sitter::Node<'a>,
}

pub fn by_position(input: &Input<'_, '_>, matching: &mut Map) {
    let SiblingNodes {
        ids: a_ids,
        nodes: nodes_a,
    } = input.a;
    let SiblingNodes {
        ids: b_ids,
        nodes: nodes_b,
    } = input.b;

    let a_by_parent = build_parent_index(&ParentIndexInput {
        ids: a_ids,
        nodes: nodes_a,
        root_id: input.root_a.id(),
        matching_filter: |id| matching.is_matched_a(id),
    });
    let b_by_parent = build_parent_index(&ParentIndexInput {
        ids: b_ids,
        nodes: nodes_b,
        root_id: input.root_b.id(),
        matching_filter: |id| matching.is_matched_b(id),
    });

    for (a_parent_key, a_children) in &a_by_parent {
        let b_parent_key = match *a_parent_key {
            ROOT_SENTINEL => ROOT_SENTINEL,
            ROOT_CHILDREN_SENTINEL => ROOT_CHILDREN_SENTINEL,
            parent_id => match matching.get_match_a_to_b(parent_id) {
                Some(b_id) => b_id,
                None => continue,
            },
        };

        let Some(b_children) = b_by_parent.get(&b_parent_key) else {
            continue;
        };

        let mut a_sorted: Vec<_> = a_children.clone();
        let mut b_sorted: Vec<_> = b_children.clone();
        a_sorted.sort_by_key(|e| e.child_index);
        b_sorted.sort_by_key(|e| e.child_index);

        for (a_entry, b_entry) in a_sorted.iter().zip(b_sorted.iter()) {
            if !matching.is_matched_a(a_entry.node_id) && !matching.is_matched_b(b_entry.node_id) {
                subtrees(
                    &SubtreesInput {
                        node_a: nodes_a.get(a_entry.node_id).copied(),
                        node_b: nodes_b.get(b_entry.node_id).copied(),
                        nodes_a,
                        nodes_b,
                    },
                    matching,
                );
            }
        }
    }
}
