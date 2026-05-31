use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::{
    config::Config,
    matching::{Map, Pair},
    metadata::{DescendantRangeQuery, Index, build_kind_index, compute_node_meta},
};

/// Dice coefficient numerator multiplier: |2 * intersection| / |A| + |B|.
const DICE_NUMERATOR_FACTOR: f64 = 2.0;

/// Near-perfect Dice match threshold for early exit in candidate search.
const NEAR_PERFECT_DICE: f64 = 0.999;

struct Candidate {
    b_id: usize,
    dice: f64,
}

pub struct BottomUpInput<'a, 'b> {
    pub nodes_a: &'b [tree_sitter::Node<'a>],
    pub nodes_b: &'b [tree_sitter::Node<'a>],
    pub config: &'b Config,
}

pub fn bottom_up_phase(input: &BottomUpInput<'_, '_>, matching: &mut Map) {
    let BottomUpInput {
        nodes_a,
        nodes_b,
        config,
    } = *input;
    let meta_a = compute_node_meta(nodes_a);
    let meta_b = compute_node_meta(nodes_b);

    let kind_index_b = build_kind_index(&meta_b);

    let match_index = Index::new(matching, &meta_a, &meta_b);

    let unmatched_a: Vec<usize> = (0..nodes_a.len())
        .filter(|&id| !matching.is_matched_a(id))
        .collect();

    let matched_b: rustc_hash::FxHashSet<usize> = matching.iter().map(|pair| pair.b_id).collect();

    let new_matches: Vec<Pair> = unmatched_a
        .par_iter()
        .filter_map(|&a_id| {
            let ma = meta_a.get(a_id)?;
            debug_assert!(
                a_id < meta_a.len(),
                "a_id {a_id} must be a valid index into meta_a"
            );

            let candidates = kind_index_b.get(&ma.kind_id)?;

            let mut best: Option<Candidate> = None;

            for &b_id in candidates {
                if matched_b.contains(&b_id) {
                    continue;
                }

                let Some(mb) = meta_b.get(b_id) else {
                    debug_assert!(
                        false,
                        "b_id {b_id} from kind_index_b must be a valid index into meta_b"
                    );
                    continue;
                };

                let matched_count = match_index.count_descendants_in_range(&DescendantRangeQuery {
                    a_start: ma.start,
                    a_end: ma.end,
                    b_start: mb.start,
                    b_end: mb.end,
                });

                if ma.descendants == 0 || mb.descendants == 0 {
                    continue;
                }

                let dice = (DICE_NUMERATOR_FACTOR * f64::from(matched_count))
                    / (ma.descendants as f64 + mb.descendants as f64);

                if dice < config.dice_threshold {
                    continue;
                }

                if best.as_ref().is_none_or(|b| dice > b.dice) {
                    best = Some(Candidate { b_id, dice });
                }

                if dice >= NEAR_PERFECT_DICE {
                    break;
                }
            }

            best.map(|candidate| Pair {
                a_id,
                b_id: candidate.b_id,
            })
        })
        .collect();

    let applied = AtomicUsize::new(0);
    for pair in new_matches {
        if !matching.is_matched_a(pair.a_id) && !matching.is_matched_b(pair.b_id) {
            matching.add_match(pair.a_id, pair.b_id);
            applied.fetch_add(1, Ordering::Relaxed);
        }
    }
}
