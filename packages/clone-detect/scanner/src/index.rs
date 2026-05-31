use clone_hash::NodeInfo;
use rustc_hash::FxHashMap;

pub struct Entry {
    pub file_id: usize,
    pub node_idx: usize,
}

/// A location in the clone index: file ID and node index within that file.
#[derive(Debug, Clone, Copy)]
pub struct Location {
    pub file_id: usize,
    pub node_idx: usize,
}

/// A clone candidate: a hash and all locations that share it.
pub struct CandidateEntry<'a> {
    pub hash: &'a u64,
    pub locations: &'a Vec<Location>,
}

#[derive(Debug, Default)]
pub struct Hash {
    pub content_index: FxHashMap<u64, Vec<Location>>,
    pub normalized_index: FxHashMap<u64, Vec<Location>>,
}

impl Hash {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, entry: &Entry, node: &NodeInfo) {
        let loc = Location {
            file_id: entry.file_id,
            node_idx: entry.node_idx,
        };

        self.content_index
            .entry(node.content_hash)
            .or_default()
            .push(loc);

        self.normalized_index
            .entry(node.normalized_hash)
            .or_default()
            .push(loc);
    }

    pub fn type1_candidates(&self) -> impl Iterator<Item = CandidateEntry<'_>> {
        self.content_index
            .iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|(hash, locations)| CandidateEntry { hash, locations })
    }

    pub fn type2_candidates(&self) -> impl Iterator<Item = CandidateEntry<'_>> {
        self.normalized_index
            .iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|(hash, locations)| CandidateEntry { hash, locations })
    }
}
