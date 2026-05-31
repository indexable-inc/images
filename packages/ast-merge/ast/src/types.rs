use std::hash::{Hash, Hasher};

use rustc_hash::FxHasher;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Revision {
    Base,
    Left,
    Right,
}

impl Revision {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Base => "BASE",
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Node<'a> {
    inner: tree_sitter::Node<'a>,
    hash: u64,
    id: NodeId,
}

impl<'a> Node<'a> {
    #[must_use]
    pub fn new(node: tree_sitter::Node<'a>, id: NodeId) -> Self {
        let hash = compute_subtree_hash(node);
        Self {
            inner: node,
            hash,
            id,
        }
    }

    #[must_use]
    pub const fn node(&self) -> tree_sitter::Node<'a> {
        self.inner
    }

    #[must_use]
    pub const fn hash(&self) -> u64 {
        self.hash
    }

    #[must_use]
    pub const fn id(&self) -> NodeId {
        self.id
    }

    #[must_use]
    pub fn kind(&self) -> &'static str {
        self.inner.kind()
    }

    #[must_use]
    pub fn byte_range(&self) -> std::ops::Range<usize> {
        self.inner.byte_range()
    }

    #[must_use]
    pub fn is_named(&self) -> bool {
        self.inner.is_named()
    }

    #[must_use]
    pub fn child_count(&self) -> usize {
        self.inner.child_count()
    }
}

fn compute_subtree_hash(node: tree_sitter::Node<'_>) -> u64 {
    let mut hasher = FxHasher::default();

    node.kind().hash(&mut hasher);

    if node.child_count() == 0 {
        node.start_byte().hash(&mut hasher);
        node.end_byte().hash(&mut hasher);
    } else {
        node.child_count().hash(&mut hasher);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            compute_subtree_hash(child).hash(&mut hasher);
        }
    }

    hasher.finish()
}
