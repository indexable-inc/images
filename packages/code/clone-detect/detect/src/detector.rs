use std::{collections::BTreeSet, path::PathBuf};

use clone_scanner::{Location, Output};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    sequences::sequence_instances,
    type3::find,
    types::{CloneGroup, DetectConfig, DetectionResult, DetectionStats, Fragment, Kind},
};

/// Multiplier turning a ratio into a percentage.
const PERCENT: f64 = 100.0;

#[must_use]
pub fn instances(scan: &Output, config: &DetectConfig) -> DetectionResult {
    let mut instances = Vec::new();
    let mut seen_type1: FxHashSet<u64> = FxHashSet::default();

    for candidate in scan.index.type1_candidates() {
        if candidate.locations.len() < 2 {
            continue;
        }

        if !seen_type1.insert(*candidate.hash) {
            continue;
        }

        let fragments = locations_to_fragments(candidate.locations, scan);
        if fragments.len() >= 2 {
            instances.push(CloneGroup {
                clone_type: Kind::Type1,
                fragments,
            });
        }
    }

    for candidate in scan.index.type2_candidates() {
        if candidate.locations.len() < 2 {
            continue;
        }

        let all_type1 = candidate.locations.iter().all(|loc| {
            let node = &scan
                .files
                .get(loc.file_id)
                .and_then(|f| f.nodes.get(loc.node_idx));
            node.is_some_and(|n| seen_type1.contains(&n.content_hash))
        });

        if all_type1 {
            continue;
        }

        let fragments = locations_to_fragments(candidate.locations, scan);
        if fragments.len() >= 2 {
            instances.push(CloneGroup {
                clone_type: Kind::Type2,
                fragments,
            });
        }
    }

    let type3_groups = if config.enable_type3 {
        find(scan, config.type3_threshold, config.type3_metric)
    } else {
        Vec::new()
    };

    let sequence_groups = if config.enable_sequences {
        sequence_instances(scan, config.sequence_window_size)
    } else {
        Vec::new()
    };

    let type3_count = type3_groups.len();
    let sequence_count = sequence_groups.len();
    instances.extend(type3_groups);
    instances.extend(sequence_groups);

    dedup_subsumed(&mut instances);

    let type1_count = instances
        .iter()
        .filter(|c| matches!(c.clone_type, Kind::Type1))
        .count();
    let type2_count = instances
        .iter()
        .filter(|c| matches!(c.clone_type, Kind::Type2))
        .count();

    let total_lines: usize = scan.files.iter().map(|f| f.source.lines().count()).sum();

    let duplicated_lines = compute_duplicated_lines(&instances);

    #[expect(
        clippy::cast_precision_loss,
        reason = "line counts stay far below f64 mantissa precision"
    )]
    let duplication_pct = if total_lines == 0 {
        0.0
    } else {
        (duplicated_lines as f64 / total_lines as f64) * PERCENT
    };

    DetectionResult {
        instances,
        stats: DetectionStats {
            files_scanned: scan.files.len(),
            nodes_analyzed: scan.files.iter().map(|f| f.nodes.len()).sum(),
            total_lines,
            duplicated_lines,
            duplication_pct,
            type1_groups: type1_count,
            type2_groups: type2_count,
            type3_groups: type3_count,
            sequence_groups: sequence_count,
        },
    }
}

fn locations_to_fragments(locations: &[Location], scan: &Output) -> Vec<Fragment> {
    locations
        .iter()
        .filter_map(|loc| {
            let file = scan.files.get(loc.file_id)?;
            let node = file.nodes.get(loc.node_idx)?;
            Some(Fragment::from_node(file, node))
        })
        .collect()
}

/// Remove clone groups that are fully subsumed by a larger group.
///
/// A group B is subsumed by group A if every fragment in B is byte-range
/// contained within some fragment of A (same file). This eliminates
/// duplicate reports caused by nested AST nodes (e.g., a `function_item`
/// and its child `block` both appearing as separate clone groups).
fn dedup_subsumed(groups: &mut Vec<CloneGroup>) {
    let n = groups.len();
    if n < 2 {
        return;
    }

    // Sort groups by total byte span (largest first) so that outer groups
    // are checked as potential containers before inner groups.
    groups.sort_by_key(|g| std::cmp::Reverse(total_byte_span(g)));

    // Intern file paths to integer ids up front: the O(n^2) subsumption scan
    // below compares fragment files pairwise, and `PathBuf` equality re-walks
    // path components each time — profiling showed it dominating whole-repo
    // runs once Type-3 group counts reach the thousands. With interned ids,
    // each comparison is a single integer branch.
    let keys: Vec<Vec<FragKey>> = {
        let mut path_ids: FxHashMap<&PathBuf, usize> = FxHashMap::default();
        groups
            .iter()
            .map(|group| {
                group
                    .fragments
                    .iter()
                    .map(|frag| {
                        let next = path_ids.len();
                        FragKey {
                            file_id: *path_ids.entry(&frag.file).or_insert(next),
                            start: frag.byte_range.start,
                            end: frag.byte_range.end,
                        }
                    })
                    .collect()
            })
            .collect()
    };

    let mut subsumed = vec![false; n];

    for (i, outer) in keys.iter().enumerate() {
        let Some(&already) = subsumed.get(i) else {
            break;
        };
        if already {
            continue;
        }

        for (j, inner) in keys.iter().enumerate().skip(i + 1) {
            let Some(&already_j) = subsumed.get(j) else {
                break;
            };
            if already_j {
                continue;
            }

            if is_subsumed_by(inner, outer)
                && let Some(flag) = subsumed.get_mut(j)
            {
                *flag = true;
            }
        }
    }

    let kept: Vec<CloneGroup> = groups
        .drain(..)
        .zip(subsumed)
        .filter(|(_, is_subsumed)| !is_subsumed)
        .map(|(group, _)| group)
        .collect();
    *groups = kept;
}

/// A fragment reduced to integers for the subsumption scan: interned file id
/// plus byte range.
#[derive(Clone, Copy)]
struct FragKey {
    file_id: usize,
    start: usize,
    end: usize,
}

/// Check if every fragment in `inner` is byte-range contained within
/// some fragment of `outer` from the same file.
fn is_subsumed_by(inner: &[FragKey], outer: &[FragKey]) -> bool {
    inner.iter().all(|inner_frag| {
        outer.iter().any(|outer_frag| {
            outer_frag.file_id == inner_frag.file_id
                && outer_frag.start <= inner_frag.start
                && outer_frag.end >= inner_frag.end
        })
    })
}

/// Count deduplicated lines involved in clone fragments.
///
/// For each clone group, only count duplicated instances (all fragments except
/// one "original"). Lines are deduplicated across groups per file using a
/// set of line numbers.
fn compute_duplicated_lines(instances: &[CloneGroup]) -> usize {
    let mut dup_lines_per_file: FxHashMap<&PathBuf, BTreeSet<usize>> = FxHashMap::default();

    for group in instances {
        // Skip the first fragment (the "original"); remaining are duplicates.
        for frag in group.fragments.iter().skip(1) {
            let lines = dup_lines_per_file.entry(&frag.file).or_default();
            for line in frag.lines.start..=frag.lines.end {
                lines.insert(line);
            }
        }
    }

    dup_lines_per_file.values().map(BTreeSet::len).sum()
}

fn total_byte_span(group: &CloneGroup) -> usize {
    group
        .fragments
        .iter()
        .map(|f| f.byte_range.end.saturating_sub(f.byte_range.start))
        .sum()
}
