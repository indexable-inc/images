//! Thresholds derived from the corpus rather than from convention.
//!
//! Published thresholds are not empirical. The 1976 cyclomatic limit of 10 was
//! described by its own author as reasonable but not magical, and the cognitive
//! limit of 15 was reached by raising the number until the output felt less
//! noisy. The one derived method is Alves, Ypma and Visser (ICSM 2010): weight
//! each unit by its size and take the metric value
//! at which the cumulative weight crosses a chosen share of the corpus. A
//! threshold then means something checkable, "the worst N percent of this repo
//! by volume", instead of appealing to authority.

/// Coverage points reported by `complexity quantiles`, matching the ICSM 2010
/// paper's low, moderate and high risk bands.
pub const COVERAGE: &[(&str, f64)] = &[("p70", 0.70), ("p80", 0.80), ("p90", 0.90)];

/// The metric value at which cumulative size crosses `coverage` of the total.
///
/// `samples` is `(metric, lines)`, and is sorted in place. Returns `None` for
/// an empty corpus.
#[must_use]
pub fn threshold(samples: &mut [(u32, usize)], coverage: f64) -> Option<u32> {
    samples.sort_unstable();
    let total: usize = samples.iter().map(|(_, lines)| *lines).sum();
    #[expect(
        clippy::cast_precision_loss,
        reason = "a corpus large enough to lose precision here is orders of magnitude beyond any repo"
    )]
    let target = total as f64 * coverage;
    let mut cumulative = 0usize;
    for (metric, lines) in samples.iter() {
        cumulative += lines;
        #[expect(
            clippy::cast_precision_loss,
            reason = "same bound as the target computation above"
        )]
        let reached = cumulative as f64 >= target;
        if reached {
            return Some(*metric);
        }
    }
    samples.last().map(|(metric, _)| *metric)
}

#[cfg(test)]
mod tests {
    use super::threshold;

    /// The threshold is weighted by size, so one large unit outranks many
    /// small ones. Without the weighting a repo of one-line helpers would set
    /// the bar for its thousand-line modules.
    #[test]
    fn weights_by_size_not_by_count() {
        let mut by_count = vec![(1, 1), (1, 1), (1, 1), (50, 1)];
        let mut by_size = vec![(1, 1), (1, 1), (1, 1), (50, 200)];

        assert_eq!(threshold(&mut by_count, 0.9), Some(50));
        assert_eq!(threshold(&mut by_size, 0.5), Some(50));
        assert_eq!(threshold(&mut by_size, 0.01), Some(1));
    }

    #[test]
    fn empty_corpus_has_no_threshold() {
        assert_eq!(threshold(&mut [], 0.9), None);
    }
}
