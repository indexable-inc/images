/// A run of consecutive equal elements in a sorted slice.
struct Run {
    value: u64,
    count: u32,
}

/// Multiset overlap counts from a two-pointer merge of two sorted slices.
struct Overlap {
    /// `|A ∩ B|` counted with multiplicity (`sum of min(count_a, count_b)`).
    intersection: u32,
    /// `|A ∪ B|` counted with multiplicity (`sum of max(count_a, count_b)`).
    union: u32,
    /// `|A|` = total element count of `a` (with multiplicity).
    len_a: u32,
    /// `|B|` = total element count of `b` (with multiplicity).
    len_b: u32,
}

/// Merge two pre-sorted multisets, accumulating intersection, union, and sizes
/// in a single O(n+m) pass. Zero allocation, cache-friendly.
///
/// Every downstream similarity metric (Jaccard, overlap coefficient) is a ratio
/// of these four counts, so we compute them once and let callers pick the ratio.
fn merge_sorted(a: &[u64], b: &[u64]) -> Overlap {
    debug_assert!(a.is_sorted(), "merge_sorted: a must be sorted");
    debug_assert!(b.is_sorted(), "merge_sorted: b must be sorted");

    let mut intersection = 0u32;
    let mut union = 0u32;
    let mut i = 0;
    let mut j = 0;

    while i < a.len() && j < b.len() {
        let run_a = run_length(a, i);
        let run_b = run_length(b, j);

        match run_a.value.cmp(&run_b.value) {
            std::cmp::Ordering::Less => {
                union += run_a.count;
                i += run_a.count as usize;
            }
            std::cmp::Ordering::Greater => {
                union += run_b.count;
                j += run_b.count as usize;
            }
            std::cmp::Ordering::Equal => {
                intersection += run_a.count.min(run_b.count);
                union += run_a.count.max(run_b.count);
                i += run_a.count as usize;
                j += run_b.count as usize;
            }
        }
    }

    // Remaining elements in whichever slice is not exhausted contribute only
    // to the union (no counterpart to intersect with).
    while i < a.len() {
        let run = run_length(a, i);
        union += run.count;
        i += run.count as usize;
    }
    while j < b.len() {
        let run = run_length(b, j);
        union += run.count;
        j += run.count as usize;
    }

    // Widening to u32 is safe: AST feature counts are far below u32::MAX.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "AST feature counts are far below u32::MAX"
    )]
    Overlap {
        intersection,
        union,
        len_a: a.len() as u32,
        len_b: b.len() as u32,
    }
}

/// Compute exact multiset **Jaccard** similarity from pre-sorted feature slices:
/// `|A ∩ B| / |A ∪ B|`.
#[must_use]
pub fn multiset_sorted(a: &[u64], b: &[u64]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }

    let o = merge_sorted(a, b);
    if o.union == 0 {
        return 0.0;
    }
    f64::from(o.intersection) / f64::from(o.union)
}

/// Compute the multiset **overlap coefficient** (containment) from pre-sorted
/// feature slices: `|A ∩ B| / min(|A|, |B|)`.
///
/// Unlike Jaccard, this does not penalize size differences: if the smaller
/// multiset is (nearly) contained in the larger one, similarity stays high.
/// That is exactly the "copy-paste then insert/delete a few statements" case
/// (moderately Type-3 in `BigCloneBench` terms), where symmetric Jaccard drops
/// below threshold purely because the edited clone grew or shrank. Sherlock's
/// N-overlap result (IEEE TC 2019) found overlap outperforms Jaccard for source
/// similarity for this reason.
#[must_use]
pub fn overlap_sorted(a: &[u64], b: &[u64]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let o = merge_sorted(a, b);
    let min_len = o.len_a.min(o.len_b);
    if min_len == 0 {
        return 0.0;
    }
    f64::from(o.intersection) / f64::from(min_len)
}

/// Count consecutive equal elements starting at `start` in a sorted slice.
///
/// Callers guarantee `start < slice.len()` (guarded by the `while i < a.len()`
/// loop condition in [`merge_sorted`]).
#[inline]
fn run_length(slice: &[u64], start: usize) -> Run {
    let Some(&value) = slice.get(start) else {
        return Run { value: 0, count: 0 };
    };
    let mut count = 1u32;
    while let Some(&next) = slice.get(start + count as usize) {
        if next != value {
            break;
        }
        count += 1;
    }
    Run { value, count }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical() {
        let sim = multiset_sorted(&[1, 2, 3], &[1, 2, 3]);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn disjoint() {
        let sim = multiset_sorted(&[1, 2, 3], &[4, 5, 6]);
        assert!(sim.abs() < 0.001);
    }

    #[test]
    fn with_duplicates() {
        let sim = multiset_sorted(&[1, 1, 2], &[1, 2, 2]);
        assert!((sim - 0.5).abs() < 0.001);
    }

    #[test]
    fn empty() {
        assert!(multiset_sorted(&[], &[]).abs() < 0.001);
        assert!(multiset_sorted(&[1, 2], &[]).abs() < 0.001);
    }

    #[test]
    fn overlap_identical() {
        let sim = overlap_sorted(&[1, 2, 3], &[1, 2, 3]);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn overlap_disjoint() {
        let sim = overlap_sorted(&[1, 2, 3], &[4, 5, 6]);
        assert!(sim.abs() < 0.001);
    }

    #[test]
    fn overlap_empty() {
        assert!(overlap_sorted(&[], &[]).abs() < 0.001);
        assert!(overlap_sorted(&[1, 2], &[]).abs() < 0.001);
    }

    /// The defining asymmetry: a small multiset fully contained in a larger one.
    /// `A = {1,2,3}`, `B = {1,2,3,4,5,6}`. Intersection = 3.
    /// Jaccard = 3/6 = 0.5 (penalizes the size gap); overlap = 3/min(3,6) = 1.0.
    #[test]
    fn overlap_beats_jaccard_on_containment() {
        let a = [1, 2, 3];
        let b = [1, 2, 3, 4, 5, 6];
        let jac = multiset_sorted(&a, &b);
        let ovl = overlap_sorted(&a, &b);
        assert!((jac - 0.5).abs() < 0.001, "jaccard = {jac}");
        assert!((ovl - 1.0).abs() < 0.001, "overlap = {ovl}");
        assert!(ovl > jac, "overlap must exceed jaccard under containment");
    }

    /// Overlap is symmetric in its arguments (min is order-independent).
    #[test]
    fn overlap_symmetric() {
        let a = [1, 2, 3];
        let b = [1, 2, 3, 4, 5, 6];
        let ab = overlap_sorted(&a, &b);
        let ba = overlap_sorted(&b, &a);
        assert!((ab - ba).abs() < 0.001);
    }
}
