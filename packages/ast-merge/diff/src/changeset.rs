use ast_merge_ast::Revision;
use rustc_hash::{FxHashMap, FxHashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PcsTriple {
    pub parent: Option<usize>,
    pub predecessor: Option<usize>,
    pub successor: usize,
}

impl PcsTriple {
    #[must_use]
    pub const fn new(parent: Option<usize>, predecessor: Option<usize>, successor: usize) -> Self {
        Self {
            parent,
            predecessor,
            successor,
        }
    }
}

#[derive(Debug, Default)]
pub struct ChangeSet {
    triples: FxHashMap<PcsTriple, FxHashSet<Revision>>,
}

/// A reference to a PCS triple and its associated revision set.
pub struct ChangeSetEntry<'a> {
    pub triple: &'a PcsTriple,
    pub revisions: &'a FxHashSet<Revision>,
}

impl ChangeSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, triple: PcsTriple, revision: Revision) {
        self.triples.entry(triple).or_default().insert(revision);
    }

    #[must_use]
    pub fn get_revisions(&self, triple: &PcsTriple) -> Option<&FxHashSet<Revision>> {
        self.triples.get(triple)
    }

    #[must_use]
    pub fn contains(&self, triple: &PcsTriple) -> bool {
        self.triples.contains_key(triple)
    }

    pub fn iter(&self) -> impl Iterator<Item = ChangeSetEntry<'_>> {
        self.triples
            .iter()
            .map(|(triple, revisions)| ChangeSetEntry { triple, revisions })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.triples.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.triples.is_empty()
    }
}
