/// A run of consecutive equal elements in a sorted slice.
struct Run {
    value: u64,
    count: u32,
}

/// Compute exact multiset Jaccard similarity from pre-sorted feature slices.
///
/// Uses a two-pointer merge — zero allocation, cache-friendly O(n+m).
#[must_use]
pub fn multiset_sorted(a: &[u64], b: &[u64]) -> f64 {
    debug_assert!(a.is_sorted(), "multiset_sorted: a must be sorted");
    debug_assert!(b.is_sorted(), "multiset_sorted: b must be sorted");

    if a.is_empty() && b.is_empty() {
        return 0.0;
    }

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

    // Remaining elements in whichever slice is not exhausted
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

    if union == 0 {
        return 0.0;
    }

    f64::from(intersection) / f64::from(union)
}

/// Count consecutive equal elements starting at `start` in a sorted slice.
///
/// Callers guarantee `start < slice.len()` (guarded by the `while i < a.len()`
/// loop condition in `multiset_sorted`).
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
}
