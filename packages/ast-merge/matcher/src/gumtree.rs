mod bottomup;
mod siblings;

use ast_merge_ast::{Tree, compute};
use rustc_hash::FxHashMap;

use crate::{
    config::Config,
    matching::Map,
    traverse::{SiblingNodes, SubtreesInput, assign_node_ids, node_height, subtrees},
};

pub struct GumTree<'a> {
    tree_a: &'a Tree,
    tree_b: &'a Tree,
    config: Config,
}

struct NodePairSlices<'a, 'b> {
    nodes_a: &'b [tree_sitter::Node<'a>],
    nodes_b: &'b [tree_sitter::Node<'a>],
    matching: &'b mut Map,
}

impl<'a> GumTree<'a> {
    #[must_use]
    pub const fn new(tree_a: &'a Tree, tree_b: &'a Tree, config: Config) -> Self {
        Self {
            tree_a,
            tree_b,
            config,
        }
    }

    #[must_use]
    pub fn compute(&self) -> Map {
        let mut matching = Map::new();

        let mut nodes_a = Vec::new();
        let mut nodes_b = Vec::new();
        assign_node_ids(self.tree_a.root_node(), &mut nodes_a);
        assign_node_ids(self.tree_b.root_node(), &mut nodes_b);

        tracing::debug!(
            nodes_a = nodes_a.len(),
            nodes_b = nodes_b.len(),
            "tree sizes"
        );

        if let (Some(root_a), Some(root_b)) = (nodes_a.first(), nodes_b.first())
            && root_a.kind_id() == root_b.kind_id()
        {
            matching.add_match(0, 0);
        }

        let phase1_start = std::time::Instant::now();
        self.top_down_phase(NodePairSlices {
            nodes_a: &nodes_a,
            nodes_b: &nodes_b,
            matching: &mut matching,
        });
        tracing::debug!(
            elapsed_ms = phase1_start.elapsed().as_millis(),
            matches = matching.len(),
            "top_down_phase complete"
        );

        let leaf_phase_start = std::time::Instant::now();
        self.leaf_sibling_phase(NodePairSlices {
            nodes_a: &nodes_a,
            nodes_b: &nodes_b,
            matching: &mut matching,
        });
        tracing::debug!(
            elapsed_ms = leaf_phase_start.elapsed().as_millis(),
            matches = matching.len(),
            "leaf_sibling_phase complete"
        );

        let phase2_start = std::time::Instant::now();
        bottomup::bottom_up_phase(
            &bottomup::BottomUpInput {
                nodes_a: &nodes_a,
                nodes_b: &nodes_b,
                config: &self.config,
            },
            &mut matching,
        );
        tracing::debug!(
            elapsed_ms = phase2_start.elapsed().as_millis(),
            matches = matching.len(),
            "bottom_up_phase complete"
        );

        matching
    }

    fn top_down_phase(&self, slices: NodePairSlices<'a, '_>) {
        let NodePairSlices {
            nodes_a,
            nodes_b,
            matching,
        } = slices;

        let mut hash_to_a: FxHashMap<u64, Vec<usize>> = FxHashMap::default();
        let mut hash_to_b: FxHashMap<u64, Vec<usize>> = FxHashMap::default();

        for (id, &node) in nodes_a.iter().enumerate() {
            if node_height(node) >= self.config.min_height {
                let hash = compute(self.tree_a, node);
                hash_to_a.entry(hash).or_default().push(id);
            }
        }

        for (id, &node) in nodes_b.iter().enumerate() {
            if node_height(node) >= self.config.min_height {
                let hash = compute(self.tree_b, node);
                hash_to_b.entry(hash).or_default().push(id);
            }
        }

        for (hash, a_ids) in &hash_to_a {
            let Some(b_ids) = hash_to_b.get(hash) else {
                continue;
            };

            if let ([a_id], [b_id]) = (a_ids.as_slice(), b_ids.as_slice()) {
                let a_id = *a_id;
                let b_id = *b_id;

                if matching.is_matched_a(a_id) || matching.is_matched_b(b_id) {
                    continue;
                }

                subtrees(
                    &SubtreesInput {
                        node_a: nodes_a.get(a_id).copied(),
                        node_b: nodes_b.get(b_id).copied(),
                        nodes_a,
                        nodes_b,
                    },
                    matching,
                );
            } else {
                siblings::by_position(
                    &siblings::Input {
                        a: SiblingNodes {
                            ids: a_ids,
                            nodes: nodes_a,
                        },
                        b: SiblingNodes {
                            ids: b_ids,
                            nodes: nodes_b,
                        },
                        root_a: self.tree_a.root_node(),
                        root_b: self.tree_b.root_node(),
                    },
                    matching,
                );
            }
        }
    }

    fn leaf_sibling_phase(&self, slices: NodePairSlices<'a, '_>) {
        let NodePairSlices {
            nodes_a,
            nodes_b,
            matching,
        } = slices;

        let mut hash_to_a: FxHashMap<u64, Vec<usize>> = FxHashMap::default();
        let mut hash_to_b: FxHashMap<u64, Vec<usize>> = FxHashMap::default();

        for (id, &node) in nodes_a.iter().enumerate() {
            if matching.is_matched_a(id) {
                continue;
            }
            if node_height(node) < self.config.min_height {
                let hash = compute(self.tree_a, node);
                hash_to_a.entry(hash).or_default().push(id);
            }
        }

        for (id, &node) in nodes_b.iter().enumerate() {
            if matching.is_matched_b(id) {
                continue;
            }
            if node_height(node) < self.config.min_height {
                let hash = compute(self.tree_b, node);
                hash_to_b.entry(hash).or_default().push(id);
            }
        }

        for (hash, a_ids) in &hash_to_a {
            let Some(b_ids) = hash_to_b.get(hash) else {
                continue;
            };

            if let (&[a_id], &[b_id]) = (a_ids.as_slice(), b_ids.as_slice()) {
                if !matching.is_matched_a(a_id) && !matching.is_matched_b(b_id) {
                    matching.add_match(a_id, b_id);
                }
            } else {
                siblings::by_position(
                    &siblings::Input {
                        a: SiblingNodes {
                            ids: a_ids,
                            nodes: nodes_a,
                        },
                        b: SiblingNodes {
                            ids: b_ids,
                            nodes: nodes_b,
                        },
                        root_a: self.tree_a.root_node(),
                        root_b: self.tree_b.root_node(),
                    },
                    matching,
                );
            }
        }
    }
}
