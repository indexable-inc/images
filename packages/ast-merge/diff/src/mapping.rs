use ast_merge_ast::Revision;
use rustc_hash::FxHashMap;

/// A node identified by its revision and ID within that revision.
#[derive(Clone, Copy)]
pub struct RevisionNode {
    pub revision: Revision,
    pub node_id: usize,
}

/// A pair of revision-nodes to merge into the same equivalence class.
pub struct RevisionNodePair {
    pub a: RevisionNode,
    pub b: RevisionNode,
}

/// Key for looking up a node's equivalence class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RevisionNodeKey {
    revision: Revision,
    node_id: usize,
}

#[derive(Debug, Default)]
pub struct Class {
    node_classes: FxHashMap<RevisionNodeKey, usize>,
    members: FxHashMap<usize, FxHashMap<Revision, usize>>,
    next: usize,
}

impl Class {
    /// Three-way merge has exactly 3 revisions: Base, Left, Right.
    const REVISION_COUNT: usize = 3;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_singleton(&mut self, node: RevisionNode) -> usize {
        let class_id = self.next;
        self.next += 1;

        self.node_classes.insert(
            RevisionNodeKey {
                revision: node.revision,
                node_id: node.node_id,
            },
            class_id,
        );
        let mut members = FxHashMap::default();
        members.insert(node.revision, node.node_id);
        self.members.insert(class_id, members);

        class_id
    }

    pub fn merge(&mut self, pair: &RevisionNodePair) {
        let class_a = self
            .node_classes
            .get(&RevisionNodeKey {
                revision: pair.a.revision,
                node_id: pair.a.node_id,
            })
            .copied();
        let class_b = self
            .node_classes
            .get(&RevisionNodeKey {
                revision: pair.b.revision,
                node_id: pair.b.node_id,
            })
            .copied();

        match (class_a, class_b) {
            (Some(ca), Some(cb)) if ca == cb => {}
            (Some(ca), Some(cb)) => {
                self.unify_classes(ca, cb);
            }
            (Some(ca), None) => {
                self.add_to_class(ca, pair.b);
            }
            (None, Some(cb)) => {
                self.add_to_class(cb, pair.a);
            }
            (None, None) => {
                self.create_class_with_both(pair);
            }
        }
    }

    fn unify_classes(&mut self, class_a: usize, class_b: usize) {
        let Some(members_b) = self.members.remove(&class_b) else {
            return;
        };

        for (rev, node_id) in members_b {
            self.node_classes.insert(
                RevisionNodeKey {
                    revision: rev,
                    node_id,
                },
                class_a,
            );
            self.members
                .entry(class_a)
                .or_default()
                .insert(rev, node_id);
        }
    }

    fn add_to_class(&mut self, class_id: usize, node: RevisionNode) {
        self.node_classes.insert(
            RevisionNodeKey {
                revision: node.revision,
                node_id: node.node_id,
            },
            class_id,
        );
        self.members
            .entry(class_id)
            .or_default()
            .insert(node.revision, node.node_id);
    }

    fn create_class_with_both(&mut self, pair: &RevisionNodePair) {
        let class_id = self.next;
        self.next += 1;

        self.node_classes.insert(
            RevisionNodeKey {
                revision: pair.a.revision,
                node_id: pair.a.node_id,
            },
            class_id,
        );
        self.node_classes.insert(
            RevisionNodeKey {
                revision: pair.b.revision,
                node_id: pair.b.node_id,
            },
            class_id,
        );

        let mut members = FxHashMap::default();
        members.insert(pair.a.revision, pair.a.node_id);
        members.insert(pair.b.revision, pair.b.node_id);
        self.members.insert(class_id, members);
    }

    #[must_use]
    pub fn get_class(&self, node: RevisionNode) -> Option<usize> {
        self.node_classes
            .get(&RevisionNodeKey {
                revision: node.revision,
                node_id: node.node_id,
            })
            .copied()
    }

    #[must_use]
    pub fn get_class_members(&self, class_id: usize) -> Option<&FxHashMap<Revision, usize>> {
        self.members.get(&class_id)
    }

    #[must_use]
    pub fn is_in_all_revisions(&self, class_id: usize) -> bool {
        self.members
            .get(&class_id)
            .is_some_and(|members| members.len() == Self::REVISION_COUNT)
    }
}
