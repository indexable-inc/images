use clone_hash::NodeInfo;
use clone_scanner::Output;
use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::{
    jaccard::{multiset_sorted, overlap_sorted},
    lsh::{
        LshEntry, LshIndex, NodeLocation, banding_for_threshold, estimated_jaccard,
        estimated_overlap, minhash_signature,
    },
    types::{CloneGroup, Fragment, Kind, Type3Metric},
};

/// Margin for the `MinHash` estimate pre-check. The estimates approximate the
/// set-based metric while confirmation is multiset-based, so we allow slack and
/// only prune what is clearly below threshold.
const ESTIMATION_MARGIN: f64 = 0.1;

/// Size-compatibility floor for the overlap metric: `min(|A|, |B|) / max(|A|,
/// |B|)` must be at least this (fragments within 2.5x of each other).
///
/// Overlap divides by the smaller multiset, so without a size bound a tiny
/// generic fragment whose features happen to be swallowed by a much larger
/// node scores 1.0 — a false positive — and, worse, defeats the `MinHash`
/// pre-check: `estimated_overlap` scales the Jaccard estimate by
/// `(|A|+|B|)/min`, so at high size skew a single noisy matching slot (1/64)
/// saturates the estimate and every such pair pays the exact merge (measured
/// as a timeout over this repo versus ~1s with the floor).
///
/// 0.4 admits an edited copy that grew up to 2.5x — well beyond the
/// "insert a few statements" clones overlap targets (`BigCloneBench` MT3) —
/// while excluding the degenerate containment pairs.
const OVERLAP_SIZE_RATIO_FLOOR: f64 = 0.4;

/// A scanned node with its position in the file list.
struct IndexedNode<'a> {
    file_id: usize,
    node_idx: usize,
    node: &'a NodeInfo,
}

pub fn find(scan: &Output, threshold: f64, metric: Type3Metric) -> Vec<CloneGroup> {
    // Banding derived once from the threshold: shared by every kind group so the
    // LSH S-curve matches the configured similarity floor (see
    // `banding_for_threshold`).
    let banding = banding_for_threshold(banding_threshold(threshold, metric));

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
            let mut lens: FxHashMap<NodeLocation, FeatureLens> = FxHashMap::default();
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
                .map(|indexed| {
                    let location = NodeLocation {
                        file_id: indexed.file_id,
                        node_idx: indexed.node_idx,
                    };
                    lens.insert(
                        location,
                        FeatureLens {
                            multiset: indexed.node.subtree_features.len(),
                            set: distinct_count(&indexed.node.subtree_features),
                        },
                    );
                    LshEntry {
                        location,
                        signature: minhash_signature(&indexed.node.subtree_features),
                    }
                })
                .collect();

            if entries.len() < 2 {
                return Vec::new();
            }

            let index = LshIndex::build(&entries, banding);
            let mut groups = Vec::new();

            for pair in index.candidate_pairs() {
                // Fast pre-check: estimate the confirmation metric from the
                // MinHash signatures and prune clearly-too-dissimilar pairs
                // before the exact O(n+m) merge. This prune carries the whole
                // pipeline's performance: without it every LSH candidate pays
                // the exact merge, which measured >500x slower over this repo.
                //
                // The estimate must match the metric. Overlap >= Jaccard
                // (containment), so pruning overlap candidates on a Jaccard
                // estimate would silently drop the very insert/delete clones
                // overlap exists to catch; `estimated_overlap` reconstructs
                // containment from the Jaccard estimate plus exact sizes. The
                // MinHash signature sees a set, so the estimate uses distinct
                // counts; the size floor guards the exact (multiset) metric
                // domain, so it uses multiset counts.
                if let (Some(sig_a), Some(sig_b)) =
                    (index.signature(&pair.first), index.signature(&pair.second))
                {
                    let estimate = match metric {
                        Type3Metric::Jaccard => estimated_jaccard(sig_a, sig_b),
                        Type3Metric::Overlap => {
                            let (Some(lens_a), Some(lens_b)) =
                                (lens.get(&pair.first), lens.get(&pair.second))
                            else {
                                continue;
                            };
                            if !size_compatible(lens_a.multiset, lens_b.multiset) {
                                continue;
                            }
                            estimated_overlap(sig_a, sig_b, lens_a.set, lens_b.set)
                        }
                    };
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
                        metric,
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

/// The Jaccard level the LSH banding must be tuned to for a given metric.
///
/// LSH candidate generation is MinHash-based, so its S-curve lives in Jaccard
/// space regardless of the confirmation metric. For Jaccard that is the
/// threshold itself. For overlap, a pair can clear the overlap threshold `t`
/// while its Jaccard is as low as the fully-contained, maximally size-gapped
/// case: `I = t·min` and `max = min / r` (with `r` the size floor) give
/// `J = t·min / (min + min/r - t·min) = t / (1 + 1/r - t)`. Banding on `t`
/// directly puts the inflection far above that floor and silently drops the
/// very size-gapped clones the overlap mode exists for (e.g. at `t = 0.8`,
/// `r = 0.4`: J can be 0.296 while the t-derived inflection sits at 0.77).
fn banding_threshold(threshold: f64, metric: Type3Metric) -> f64 {
    match metric {
        Type3Metric::Jaccard => threshold,
        Type3Metric::Overlap => {
            threshold / (1.0 + 1.0 / OVERLAP_SIZE_RATIO_FLOOR - threshold)
        }
    }
}

/// Multiset and distinct feature counts for one node, precomputed per LSH entry
/// so the per-candidate estimate stays O(1).
#[derive(Clone, Copy)]
struct FeatureLens {
    /// Total feature count with multiplicity (the exact metrics' domain).
    multiset: usize,
    /// Distinct feature count (the `MinHash` estimate's domain: signatures see
    /// sets, so the overlap estimate must use set sizes or duplicate-heavy
    /// fragments get under-estimated and wrongly pruned).
    set: usize,
}

/// Count distinct values in a sorted slice (one linear pass over runs).
fn distinct_count(sorted: &[u64]) -> usize {
    let mut count = usize::from(!sorted.is_empty());
    for window in sorted.windows(2) {
        if let [a, b] = window
            && a != b
        {
            count += 1;
        }
    }
    count
}

/// Enforce [`OVERLAP_SIZE_RATIO_FLOOR`] on a pair of feature counts.
#[expect(
    clippy::cast_precision_loss,
    reason = "feature counts are far below f64 mantissa precision"
)]
fn size_compatible(len_a: usize, len_b: usize) -> bool {
    let min = len_a.min(len_b) as f64;
    let max = len_a.max(len_b) as f64;
    max > 0.0 && min / max >= OVERLAP_SIZE_RATIO_FLOOR
}

struct CandidatePair {
    loc_a: NodeLocation,
    loc_b: NodeLocation,
    threshold: f64,
    metric: Type3Metric,
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

    // Ground-truth enforcement of the overlap size floor (the candidate loop
    // also prunes on it, but only inside the estimate fast path).
    if pair.metric == Type3Metric::Overlap
        && !size_compatible(
            node_a.subtree_features.len(),
            node_b.subtree_features.len(),
        )
    {
        return None;
    }

    let similarity = compute_similarity_with(node_a, node_b, pair.metric);
    if similarity < pair.threshold {
        return None;
    }

    Some(CloneGroup {
        clone_type: Kind::Type3 {
            similarity,
            metric: pair.metric,
        },
        fragments: vec![
            Fragment::from_node(file_a, node_a),
            Fragment::from_node(file_b, node_b),
        ],
    })
}

/// Compute structural similarity between two AST nodes using the default
/// (Jaccard) metric. Kept for external callers; new code should prefer
/// [`compute_similarity_with`] to be explicit about the metric.
#[must_use]
pub fn compute_similarity(a: &NodeInfo, b: &NodeInfo) -> f64 {
    compute_similarity_with(a, b, Type3Metric::default())
}

/// Compute structural similarity between two AST nodes under `metric`.
///
/// When both nodes carry subtree features, the score is the chosen multiset
/// metric over those features. Otherwise (features unavailable) both metrics
/// fall back to the node-count ratio `min/max` — a coarse structural bound,
/// not a feature comparison, so there is nothing metric-specific to compute.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "AST node counts are far below f64 mantissa precision"
)]
pub fn compute_similarity_with(a: &NodeInfo, b: &NodeInfo, metric: Type3Metric) -> f64 {
    if !a.subtree_features.is_empty() && !b.subtree_features.is_empty() {
        return match metric {
            Type3Metric::Jaccard => multiset_sorted(&a.subtree_features, &b.subtree_features),
            Type3Metric::Overlap => overlap_sorted(&a.subtree_features, &b.subtree_features),
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Jaccard mode bands on the configured threshold itself.
    #[test]
    fn banding_threshold_jaccard_is_identity() {
        let t = banding_threshold(0.7, Type3Metric::Jaccard);
        assert!((t - 0.7).abs() < 1e-9);
    }

    /// Overlap mode must band on the worst-case implied Jaccard, not the raw
    /// threshold: a true overlap-0.8 pair at the maximum allowed size gap has
    /// Jaccard `0.8 / (1 + 1/0.4 - 0.8) ~ 0.296`. Banding on 0.8 directly puts
    /// the LSH inflection at ~0.77 and silently drops such pairs before the
    /// exact check (the size-gapped clones this mode exists for).
    #[test]
    fn banding_threshold_overlap_uses_implied_jaccard_floor() {
        let t = banding_threshold(0.8, Type3Metric::Overlap);
        let expected = 0.8 / (1.0 + 1.0 / OVERLAP_SIZE_RATIO_FLOOR - 0.8);
        assert!((t - expected).abs() < 1e-9);
        assert!(t < 0.35, "implied floor must sit far below the raw threshold");
    }

    /// Distinct counting over sorted runs.
    #[test]
    fn distinct_count_over_runs() {
        assert_eq!(distinct_count(&[]), 0);
        assert_eq!(distinct_count(&[7]), 1);
        assert_eq!(distinct_count(&[1, 1, 1]), 1);
        assert_eq!(distinct_count(&[1, 1, 2, 3, 3]), 3);
    }
}
