use clone_hash::NodeInfo;
use clone_scanner::Output;
use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::{
    jaccard::multiset_sorted,
    lsh::{LshEntry, LshIndex, NodeLocation, estimated_jaccard, minhash_signature},
    types::{CloneGroup, Fragment, Kind},
};

/// Margin for the MinHash estimate pre-check. The estimate approximates
/// set Jaccard which can differ from multiset Jaccard, so we allow some slack.
const ESTIMATION_MARGIN: f64 = 0.1;

/// A scanned node with its position in the file list.
struct IndexedNode<'a> {
    file_id: usize,
    node_idx: usize,
    node: &'a NodeInfo,
}

pub fn find(scan: &Output, threshold: f64) -> Vec<CloneGroup> {
    let mut by_kind: FxHashMap<&str, Vec<IndexedNode<'_>>> = FxHashMap::default();

    for (file_id, file) in scan.files.iter().enumerate() {
        for (node_idx, node) in file.nodes.iter().enumerate() {
            by_kind.entry(node.kind).or_default().push(IndexedNode {
                file_id,
                node_idx,
                node,
            });
        }
    }

    let kind_groups: Vec<_> = by_kind.into_iter().collect();

    kind_groups
        .par_iter()
        .flat_map(|(_kind, nodes)| {
            if nodes.len() < 2 {
                return Vec::new();
            }

            // Deduplicate by normalized_hash: nodes with the same normalized_hash
            // are already caught as Type-2 instances. Only send one representative
            // per unique normalized_hash into LSH to shrink bucket sizes.
            let mut seen_normalized: FxHashMap<u64, usize> = FxHashMap::default();
            let entries: Vec<LshEntry> = nodes
                .iter()
                .filter(|indexed| !indexed.node.subtree_features.is_empty())
                .filter(|indexed| {
                    let count = seen_normalized
                        .entry(indexed.node.normalized_hash)
                        .or_insert(0);
                    *count += 1;
                    // Keep only the first occurrence of each normalized_hash
                    *count == 1
                })
                .map(|indexed| LshEntry {
                    location: NodeLocation {
                        file_id: indexed.file_id,
                        node_idx: indexed.node_idx,
                    },
                    signature: minhash_signature(&indexed.node.subtree_features),
                })
                .collect();

            if entries.len() < 2 {
                return Vec::new();
            }

            let index = LshIndex::build(&entries);
            let mut groups = Vec::new();

            for pair in index.candidate_pairs() {
                // Fast pre-check: estimate Jaccard from MinHash signatures
                if let (Some(sig_a), Some(sig_b)) =
                    (index.signature(&pair.first), index.signature(&pair.second))
                {
                    let estimate = estimated_jaccard(sig_a, sig_b);
                    if estimate < threshold - ESTIMATION_MARGIN {
                        continue;
                    }
                }

                let Some(group) = try_make_group(
                    scan,
                    &CandidatePair {
                        loc_a: pair.first,
                        loc_b: pair.second,
                        threshold,
                    },
                ) else {
                    continue;
                };
                groups.push(group);
            }

            groups
        })
        .collect()
}

struct CandidatePair {
    loc_a: NodeLocation,
    loc_b: NodeLocation,
    threshold: f64,
}

/// Try to build a Type-3 clone group from two node locations.
/// Returns `None` if they're already Type-1/Type-2 or below threshold.
fn try_make_group(scan: &Output, pair: &CandidatePair) -> Option<CloneGroup> {
    let file_a = scan.files.get(pair.loc_a.file_id)?;
    let file_b = scan.files.get(pair.loc_b.file_id)?;
    let node_a = file_a.nodes.get(pair.loc_a.node_idx)?;
    let node_b = file_b.nodes.get(pair.loc_b.node_idx)?;

    // Skip pairs already caught as Type-1 or Type-2
    if node_a.content_hash == node_b.content_hash
        || node_a.normalized_hash == node_b.normalized_hash
    {
        return None;
    }

    let similarity = compute_similarity(node_a, node_b);
    if similarity < pair.threshold {
        return None;
    }

    Some(CloneGroup {
        clone_type: Kind::Type3 { similarity },
        fragments: vec![
            Fragment::from_node(file_a, node_a),
            Fragment::from_node(file_b, node_b),
        ],
    })
}

/// Compute structural similarity between two AST nodes.
#[must_use]
pub fn compute_similarity(a: &NodeInfo, b: &NodeInfo) -> f64 {
    if !a.subtree_features.is_empty() && !b.subtree_features.is_empty() {
        return multiset_sorted(&a.subtree_features, &b.subtree_features);
    }

    let count_a = a.node_count as f64;
    let count_b = b.node_count as f64;

    let min = count_a.min(count_b);
    let max = count_a.max(count_b);

    if max == 0.0 {
        return 0.0;
    }

    min / max
}
