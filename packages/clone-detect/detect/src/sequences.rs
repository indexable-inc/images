use std::hash::{Hash, Hasher};

use clone_scanner::Output;
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};

use crate::types::{ByteRange, CloneGroup, Fragment, Kind, LineRange};

/// Default sliding window size for statement-sequence detection.
pub const DEFAULT_WINDOW_SIZE: usize = 3;

/// Minimum useful window size.
const MIN_WINDOW_SIZE: usize = 2;

/// A location within a statement sequence.
#[derive(Debug, Clone)]
struct SeqLoc {
    file_id: usize,
    node_idx: usize,
    start: usize,
    end: usize,
}

/// Identity key for a sequence location used in deduplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SeqKey {
    file_id: usize,
    node_idx: usize,
    position: usize,
}

/// A pair of matched sequence locations.
struct SeqLocPair {
    first: SeqLoc,
    second: SeqLoc,
}

/// Detect duplicated statement sequences using sliding-window k-grams.
#[must_use]
pub fn sequence_instances(scan: &Output, window_size: usize) -> Vec<CloneGroup> {
    let window_size = window_size.max(MIN_WINDOW_SIZE);
    let mut kgram_index: FxHashMap<u64, Vec<SeqLoc>> = FxHashMap::default();

    for (file_id, file) in scan.files.iter().enumerate() {
        for (node_idx, node) in file.nodes.iter().enumerate() {
            let child_hashes: Vec<u64> = node.children.iter().map(|c| c.normalized_hash).collect();

            if child_hashes.len() < window_size {
                continue;
            }

            for start in 0..=child_hashes.len() - window_size {
                let end = start + window_size;
                let kgram_hash = hash_kgram(&child_hashes, start, end);

                kgram_index.entry(kgram_hash).or_default().push(SeqLoc {
                    file_id,
                    node_idx,
                    start,
                    end,
                });
            }
        }
    }

    let mut groups = Vec::new();
    let mut used: FxHashSet<SeqKey> = FxHashSet::default();

    for locations in kgram_index.values() {
        if locations.len() < 2 {
            continue;
        }

        for (i, loc_a) in locations.iter().enumerate() {
            for loc_b in locations.iter().skip(i + 1) {
                // Skip same-node overlapping sequences
                if loc_a.file_id == loc_b.file_id
                    && loc_a.node_idx == loc_b.node_idx
                    && ranges_overlap(loc_a.start..loc_a.end, loc_b.start..loc_b.end)
                {
                    continue;
                }

                let key_a = SeqKey {
                    file_id: loc_a.file_id,
                    node_idx: loc_a.node_idx,
                    position: loc_a.start,
                };
                let key_b = SeqKey {
                    file_id: loc_b.file_id,
                    node_idx: loc_b.node_idx,
                    position: loc_b.start,
                };
                if used.contains(&key_a) || used.contains(&key_b) {
                    continue;
                }

                let extended = extend_match(scan, loc_a, loc_b);

                for pos in extended.first.start..extended.first.end {
                    used.insert(SeqKey {
                        file_id: extended.first.file_id,
                        node_idx: extended.first.node_idx,
                        position: pos,
                    });
                }
                for pos in extended.second.start..extended.second.end {
                    used.insert(SeqKey {
                        file_id: extended.second.file_id,
                        node_idx: extended.second.node_idx,
                        position: pos,
                    });
                }

                let frag_a = sequence_to_fragment(scan, &extended.first);
                let frag_b = sequence_to_fragment(scan, &extended.second);

                if let (Some(fa), Some(fb)) = (frag_a, frag_b) {
                    groups.push(CloneGroup {
                        clone_type: Kind::Sequence {
                            statements: extended.first.end - extended.first.start,
                        },
                        fragments: vec![fa, fb],
                    });
                }
            }
        }
    }

    groups
}

/// Extend a matching window in both directions as long as child hashes match.
fn extend_match(scan: &Output, loc_a: &SeqLoc, loc_b: &SeqLoc) -> SeqLocPair {
    let node_a = scan
        .files
        .get(loc_a.file_id)
        .and_then(|f| f.nodes.get(loc_a.node_idx));
    let node_b = scan
        .files
        .get(loc_b.file_id)
        .and_then(|f| f.nodes.get(loc_b.node_idx));

    debug_assert!(
        node_a.is_some(),
        "extend_match: file_id={} node_idx={} out of bounds",
        loc_a.file_id,
        loc_a.node_idx,
    );
    debug_assert!(
        node_b.is_some(),
        "extend_match: file_id={} node_idx={} out of bounds",
        loc_b.file_id,
        loc_b.node_idx,
    );

    let (Some(node_a), Some(node_b)) = (node_a, node_b) else {
        return SeqLocPair {
            first: loc_a.clone(),
            second: loc_b.clone(),
        };
    };

    let children_a: Vec<u64> = node_a.children.iter().map(|c| c.normalized_hash).collect();
    let children_b: Vec<u64> = node_b.children.iter().map(|c| c.normalized_hash).collect();

    let mut start_a = loc_a.start;
    let mut start_b = loc_b.start;
    let mut end_a = loc_a.end;
    let mut end_b = loc_b.end;

    // Extend backward
    while start_a > 0 && start_b > 0 && children_a.get(start_a - 1) == children_b.get(start_b - 1) {
        start_a -= 1;
        start_b -= 1;
    }

    // Extend forward
    while children_a.get(end_a).is_some() && children_a.get(end_a) == children_b.get(end_b) {
        end_a += 1;
        end_b += 1;
    }

    SeqLocPair {
        first: SeqLoc {
            file_id: loc_a.file_id,
            node_idx: loc_a.node_idx,
            start: start_a,
            end: end_a,
        },
        second: SeqLoc {
            file_id: loc_b.file_id,
            node_idx: loc_b.node_idx,
            start: start_b,
            end: end_b,
        },
    }
}

fn sequence_to_fragment(scan: &Output, loc: &SeqLoc) -> Option<Fragment> {
    let file = scan.files.get(loc.file_id)?;
    let node = file.nodes.get(loc.node_idx)?;

    let first_child = node.children.get(loc.start)?;
    let last_child = node.children.get(loc.end.checked_sub(1)?)?;

    Some(Fragment {
        file: file.path.clone(),
        byte_range: ByteRange {
            start: first_child.byte_range.start,
            end: last_child.byte_range.end,
        },
        lines: LineRange {
            start: first_child.start_line,
            end: last_child.end_line,
        },
        kind: node.kind.to_owned(),
    })
}

fn hash_kgram(values: &[u64], start: usize, end: usize) -> u64 {
    let mut hasher = FxHasher::default();
    for h in values.iter().skip(start).take(end - start) {
        h.hash(&mut hasher);
    }
    hasher.finish()
}

const fn ranges_overlap(a: std::ops::Range<usize>, b: std::ops::Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}
