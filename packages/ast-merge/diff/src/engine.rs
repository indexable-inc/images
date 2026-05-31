use ast_merge_ast::{Revision, Tree};
use ast_merge_matcher::Map;

use crate::{
    changeset::{ChangeSet, PcsTriple},
    conflict::{Conflict, Region, Result},
    items::reconcile_lists,
    mapping::{Class, RevisionNode, RevisionNodePair},
    trees::{collect_nodes, find_predecessor},
};

/// Three-way tree references for base, left, and right revisions.
pub struct ThreeWayTrees<'a> {
    pub base: &'a Tree,
    pub left: &'a Tree,
    pub right: &'a Tree,
}

/// Three-way node references for base, left, and right revisions.
#[derive(Clone, Copy)]
pub struct ThreeWayNodes<'a> {
    pub base: tree_sitter::Node<'a>,
    pub left: tree_sitter::Node<'a>,
    pub right: tree_sitter::Node<'a>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub marker_size: usize,
    pub diff3_style: bool,
}

/// Standard git conflict marker size (`<<<<<<<`, `=======`, `>>>>>>>`).
const DEFAULT_MARKER_SIZE: usize = 7;

impl Default for Config {
    fn default() -> Self {
        Self {
            marker_size: DEFAULT_MARKER_SIZE,
            diff3_style: true,
        }
    }
}

/// Construction parameters for [`ThreeWay`].
pub struct ThreeWayParams<'a> {
    pub trees: ThreeWayTrees<'a>,
    pub base_left_matching: Map,
    pub base_right_matching: Map,
    pub config: Config,
}

pub struct ThreeWay<'a> {
    trees: ThreeWayTrees<'a>,
    base_left_matching: Map,
    base_right_matching: Map,
}

impl<'a> ThreeWay<'a> {
    #[must_use]
    pub fn new(params: ThreeWayParams<'a>) -> Self {
        Self {
            trees: params.trees,
            base_left_matching: params.base_left_matching,
            base_right_matching: params.base_right_matching,
        }
    }

    #[must_use]
    pub fn merge(&self) -> Result {
        let class_mapping = self.build_class_mapping();

        let changeset = self.build_changeset(&class_mapping);

        let conflicts = detect_conflicts(&changeset, &class_mapping);

        if conflicts.is_empty() {
            let content = self.reconstruct_merged(&changeset, &class_mapping);
            Result::success(content)
        } else {
            // Structural conflicts detected — fall back to line-based merge
            // which produces correct Git-style conflict markers with full
            // source text context.
            let base_src = self.trees.base.source();
            let left_src = self.trees.left.source();
            let right_src = self.trees.right.source();
            crate::lines::based(base_src, left_src, right_src)
        }
    }

    fn build_class_mapping(&self) -> Class {
        let mut mapping = Class::new();

        for pair in self.base_left_matching.iter() {
            mapping.merge(&RevisionNodePair {
                a: RevisionNode {
                    revision: Revision::Base,
                    node_id: pair.a_id,
                },
                b: RevisionNode {
                    revision: Revision::Left,
                    node_id: pair.b_id,
                },
            });
        }

        for pair in self.base_right_matching.iter() {
            mapping.merge(&RevisionNodePair {
                a: RevisionNode {
                    revision: Revision::Base,
                    node_id: pair.a_id,
                },
                b: RevisionNode {
                    revision: Revision::Right,
                    node_id: pair.b_id,
                },
            });
        }

        mapping
    }

    fn build_changeset(&self, _class_mapping: &Class) -> ChangeSet {
        let mut changeset = ChangeSet::new();

        extract_pcs_triples(self.trees.base, Revision::Base, &mut changeset);
        extract_pcs_triples(self.trees.left, Revision::Left, &mut changeset);
        extract_pcs_triples(self.trees.right, Revision::Right, &mut changeset);

        changeset
    }

    fn reconstruct_merged(&self, _changeset: &ChangeSet, _class_mapping: &Class) -> String {
        let base_items = get_top_level_items(self.trees.base);
        let left_items = get_top_level_items(self.trees.left);
        let right_items = get_top_level_items(self.trees.right);

        let items = crate::items::ThreeWay {
            base: &base_items,
            left: &left_items,
            right: &right_items,
        };
        reconcile_lists(&self.trees, &items)
    }
}

/// Detect structural conflicts in the three-way changeset.
///
/// Uses the 3DM inconsistency rule: two PCS triples conflict when they
/// share the same (parent, predecessor) but have different successors,
/// or the same (parent, successor) but different predecessors — AND
/// the conflicting triples come from different non-base revisions.
///
/// When structural conflicts are found, we don't attempt fine-grained
/// conflict regions (that requires full tree reconstruction per Mergiraf).
/// Instead we return a single conflict covering the full source text of
/// each side, letting the caller fall back to line-based merge which
/// already produces correct Git-style conflict markers.
/// (parent, predecessor) key for PCS position indexing.
struct PcsPosition {
    parent: Option<usize>,
    predecessor: Option<usize>,
}

impl std::hash::Hash for PcsPosition {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.parent.hash(state);
        self.predecessor.hash(state);
    }
}

impl PartialEq for PcsPosition {
    fn eq(&self, other: &Self) -> bool {
        self.parent == other.parent && self.predecessor == other.predecessor
    }
}

impl Eq for PcsPosition {}

/// Successor paired with the set of revisions that contain the triple.
struct SuccessorEntry<'a> {
    successor: usize,
    revisions: &'a rustc_hash::FxHashSet<Revision>,
}

#[expect(
    clippy::similar_names,
    reason = "class_{a,b}_{left,right} form a deliberate 2x2 of revision x entry"
)]
fn detect_conflicts(changeset: &ChangeSet, class_mapping: &Class) -> Vec<Conflict> {
    use rustc_hash::FxHashMap;

    // Index triples by (parent, predecessor) to find successor conflicts.
    let mut by_position: FxHashMap<PcsPosition, Vec<SuccessorEntry<'_>>> = FxHashMap::default();

    for entry in changeset.iter() {
        by_position
            .entry(PcsPosition {
                parent: entry.triple.parent,
                predecessor: entry.triple.predecessor,
            })
            .or_default()
            .push(SuccessorEntry {
                successor: entry.triple.successor,
                revisions: entry.revisions,
            });
    }

    // Check for successor conflicts: same position, different successors,
    // where one is left-only and the other is right-only.
    for entries in by_position.values() {
        if entries.len() < 2 {
            continue;
        }
        for (i, entry_a) in entries.iter().enumerate() {
            for entry_b in entries.iter().skip(i + 1) {
                if entry_a.successor == entry_b.successor {
                    continue; // Same successor = no conflict.
                }
                let a_left_only = entry_a.revisions.contains(&Revision::Left)
                    && !entry_a.revisions.contains(&Revision::Base);
                let b_right_only = entry_b.revisions.contains(&Revision::Right)
                    && !entry_b.revisions.contains(&Revision::Base);
                let a_right_only = entry_a.revisions.contains(&Revision::Right)
                    && !entry_a.revisions.contains(&Revision::Base);
                let b_left_only = entry_b.revisions.contains(&Revision::Left)
                    && !entry_b.revisions.contains(&Revision::Base);

                let conflict = (a_left_only && b_right_only) || (a_right_only && b_left_only);
                if !conflict {
                    continue;
                }
                // Check convergence: if both successors map to the same
                // equivalence class, the change is convergent (not a conflict).
                let class_a_left = class_mapping.get_class(RevisionNode {
                    revision: Revision::Left,
                    node_id: entry_a.successor,
                });
                let class_b_right = class_mapping.get_class(RevisionNode {
                    revision: Revision::Right,
                    node_id: entry_b.successor,
                });
                let class_a_right = class_mapping.get_class(RevisionNode {
                    revision: Revision::Right,
                    node_id: entry_a.successor,
                });
                let class_b_left = class_mapping.get_class(RevisionNode {
                    revision: Revision::Left,
                    node_id: entry_b.successor,
                });
                let convergent = matches!(
                    (class_a_left, class_b_right),
                    (Some(a), Some(b)) if a == b
                ) || matches!(
                    (class_a_right, class_b_left),
                    (Some(a), Some(b)) if a == b
                );
                if convergent {
                    continue;
                }

                // Structural conflict found. We don't have enough context
                // here to extract per-node conflict regions (would need the
                // original trees). Return an empty-text conflict as a signal
                // to the caller that a conflict exists.
                return vec![Conflict {
                    base: None,
                    left: Region::new(0, 0, String::new()),
                    right: Region::new(0, 0, String::new()),
                }];
            }
        }
    }

    Vec::new()
}
fn extract_pcs_triples(tree: &Tree, revision: Revision, changeset: &mut ChangeSet) {
    let mut node_ids: Vec<tree_sitter::Node<'_>> = Vec::new();
    collect_nodes(tree.root_node(), &mut node_ids);

    for (id, &node) in node_ids.iter().enumerate() {
        let parent = node
            .parent()
            .and_then(|p| node_ids.iter().position(|n| n.id() == p.id()));

        let predecessor = node
            .parent()
            .and_then(|parent_node| find_predecessor(parent_node, node, &node_ids));

        let triple = PcsTriple::new(parent, predecessor, id);
        changeset.add(triple, revision);
    }
}
fn get_top_level_items(tree: &Tree) -> Vec<tree_sitter::Node<'_>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    root.children(&mut cursor)
        .filter(|n| !n.is_extra())
        .collect()
}
