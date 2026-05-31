use rustc_hash::FxHashMap;

use crate::matching::Map;

pub struct NodeMeta {
    pub start: usize,
    pub end: usize,
    pub descendants: usize,
    pub kind_id: u16,
}

struct MatchedRange {
    a_start: usize,
    a_end: usize,
    b_start: usize,
    b_end: usize,
}

pub struct Index {
    sorted_a: Vec<MatchedRange>,
}

pub struct DescendantRangeQuery {
    pub a_start: usize,
    pub a_end: usize,
    pub b_start: usize,
    pub b_end: usize,
}

impl Index {
    pub fn new(matching: &Map, meta_a: &[NodeMeta], meta_b: &[NodeMeta]) -> Self {
        let mut sorted_a: Vec<_> = matching
            .iter()
            .filter_map(|pair| {
                let ma = meta_a.get(pair.a_id)?;
                let mb = meta_b.get(pair.b_id)?;
                Some(MatchedRange {
                    a_start: ma.start,
                    a_end: ma.end,
                    b_start: mb.start,
                    b_end: mb.end,
                })
            })
            .collect();
        sorted_a.sort_unstable_by_key(|r| r.a_start);
        Self { sorted_a }
    }

    pub fn count_descendants_in_range(&self, query: &DescendantRangeQuery) -> u32 {
        let start_idx = self.sorted_a.partition_point(|r| r.a_start < query.a_start);

        let mut count = 0u32;
        for r in self.sorted_a.get(start_idx..).unwrap_or(&[]) {
            if r.a_start >= query.a_end {
                break;
            }

            if r.a_end <= query.a_end && r.b_start >= query.b_start && r.b_end <= query.b_end {
                count += 1;
            }
        }
        count
    }
}

pub fn compute_node_meta(nodes: &[tree_sitter::Node<'_>]) -> Vec<NodeMeta> {
    let mut meta: Vec<NodeMeta> = nodes
        .iter()
        .map(|node| {
            let range = node.byte_range();
            NodeMeta {
                start: range.start,
                end: range.end,
                descendants: 1,
                kind_id: node.kind_id(),
            }
        })
        .collect();

    let counts: Vec<usize> = meta
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let mut count = 1usize;
            for m in meta.iter().skip(i + 1) {
                if m.start >= node.start && m.end <= node.end {
                    count += 1;
                }
                if m.start >= node.end {
                    break;
                }
            }
            count
        })
        .collect();

    for (m, count) in meta.iter_mut().zip(counts) {
        m.descendants = count;
    }

    meta
}

pub fn build_kind_index(meta: &[NodeMeta]) -> FxHashMap<u16, Vec<usize>> {
    let mut index: FxHashMap<u16, Vec<usize>> = FxHashMap::default();
    for (id, m) in meta.iter().enumerate() {
        index.entry(m.kind_id).or_default().push(id);
    }
    index
}
