//! In-process micro harness for timing a Rust closure and counting its
//! allocations.
//!
//! The harness produces two metrics for a benchmarked closure:
//!
//! - `wall_clock` (ns): a **distributional** metric. We run the closure in a
//!   timed loop and record one per-iteration sample per measured round, so the
//!   comparator can run a significance test rather than trusting a single mean.
//! - `allocations` (count): a **deterministic** metric, gated behind the
//!   [`CountingAllocator`] global shim. Allocation counts are reproducible for a
//!   given input, which makes them suitable as `nix flake check`s — unlike
//!   timing, which is sandbox-sensitive.
//!
//! This is a deliberately small sampler rather than a second copy of tango.
//! Tango stays the right tool for paired A/B comparison of two builds of the
//! same function in one process (see `packages/file-search/benches`); this
//! harness exists to fold a Rust closure's time and allocation behavior into the
//! same [`Run`](crate::schema::Run) schema as the macro harness and custom
//! metrics, so all three flow through one history store and one comparator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use crate::schema::Metric;

/// A `#[global_allocator]` shim that counts allocation calls.
///
/// Install it in a bench binary's crate root:
///
/// ```ignore
/// #[global_allocator]
/// static ALLOC: indexbench::micro::CountingAllocator = indexbench::micro::CountingAllocator;
/// ```
///
/// Counting is process-global and only meaningful when the measured closure runs
/// single-threaded with nothing else allocating, which is the normal shape for a
/// micro bench. The counter wraps a real [`System`] allocator, so installing it
/// changes counts but not behavior. Use [`count_allocations`] to measure a
/// closure; it snapshots the counter before and after.
pub struct CountingAllocator;

/// Number of `alloc` calls observed since process start. Bumped on every
/// allocation; `count_allocations` reads deltas, so the absolute value is not
/// meaningful on its own.
static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);

/// Live bytes outstanding (allocated minus freed). Exposed for harnesses that
/// want a peak-bytes deterministic metric; the v1 micro harness reports call
/// counts, which are the more reproducible of the two.
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method forwards to the corresponding `System` method with the
// same layout, so the allocator contract (valid pointers, matching layouts) is
// upheld by `System`. The atomics only observe; they never change which pointer
// is returned or freed.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: forwarding an unchanged layout to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        ALLOC_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: `ptr`/`layout` come straight from the caller and were produced
        // by this same `System` allocator.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        // SAFETY: forwarding caller-owned `ptr`/`layout` plus a new size.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Run `body` once and return the number of allocation calls it made.
///
/// Returns `None` when the [`CountingAllocator`] is not installed as the global
/// allocator — detected by observing that the counter did not move across a
/// probe allocation. This keeps the metric honest: a bench compiled without the
/// shim reports "no allocation data" rather than a misleading zero.
pub fn count_allocations<F: FnOnce()>(body: F) -> Option<u64> {
    // Probe: a single boxed value. If the counter advances, the shim is live.
    let before_probe = ALLOC_CALLS.load(Ordering::Relaxed);
    let probe = Box::new(0u8);
    let after_probe = ALLOC_CALLS.load(Ordering::Relaxed);
    drop(probe);
    if after_probe == before_probe {
        return None;
    }

    let before = ALLOC_CALLS.load(Ordering::Relaxed);
    body();
    let after = ALLOC_CALLS.load(Ordering::Relaxed);
    Some(after - before)
}

/// Tuning for the timed-loop sampler.
#[derive(Debug, Clone, Copy)]
pub struct TimingConfig {
    /// Unmeasured warm-up iterations run before sampling, to settle caches and
    /// the branch predictor.
    pub warmup_iters: u32,
    /// Number of measured samples (each the mean per-iteration time of a batch).
    /// The comparator wants `n >= ~8` for a meaningful significance test, so the
    /// default sits comfortably above that.
    pub samples: u32,
    /// Iterations per measured sample. Batching amortizes `Instant::now`
    /// overhead so a sub-microsecond closure still produces stable timings.
    pub batch: u32,
}

impl Default for TimingConfig {
    fn default() -> Self {
        Self {
            warmup_iters: 100,
            samples: 30,
            batch: 100,
        }
    }
}

/// The metric name the micro harness uses for its timing distribution.
///
/// Shared with the macro harness so a Rust closure and an external command both
/// report their time under one name the comparator and reporter line up on.
pub const WALL_CLOCK: &str = "wall_clock";

/// The metric name for the deterministic allocation count.
pub const ALLOCATIONS: &str = "allocations";

/// Time `body` with a warm-up then a batched sampling loop, returning a
/// distributional [`WALL_CLOCK`] metric in nanoseconds.
///
/// Each recorded sample is the mean per-iteration time over one `batch`, so the
/// returned vector has `config.samples` entries. `body` is `FnMut` so it can
/// hold per-call state (e.g. a rotating query index), matching tango's
/// `b.iter(move || ...)` shape. The metric name is fixed to `wall_clock`; the
/// bench's own name lives on the [`Run`](crate::schema::Run), not the metric.
#[expect(
    clippy::cast_precision_loss,
    reason = "per-batch nanosecond totals at bench magnitudes are far below 2^52, so the f64 timing is exact"
)]
pub fn time_fn<F: FnMut()>(config: TimingConfig, mut body: F) -> Metric {
    for _ in 0..config.warmup_iters {
        body();
    }

    let mut samples = Vec::with_capacity(config.samples as usize);
    for _ in 0..config.samples {
        let start = Instant::now();
        for _ in 0..config.batch {
            body();
        }
        let elapsed = start.elapsed();
        let per_iter = elapsed.as_nanos() as f64 / f64::from(config.batch.max(1));
        samples.push(per_iter);
    }

    Metric::distribution(WALL_CLOCK, "ns", true, samples)
}

/// Time `body` and, when the counting allocator is installed, also report a
/// deterministic [`ALLOCATIONS`] metric for one untimed call.
///
/// Returns one or two metrics: always `wall_clock`, plus `allocations` when the
/// shim is live. The allocation count comes from a single call outside the
/// timing loop so the count reflects one logical operation, not a batch.
#[expect(
    clippy::cast_precision_loss,
    reason = "an allocation count for one bench call is far below 2^52, so the f64 metric value is exact"
)]
pub fn bench_fn<F: FnMut()>(config: TimingConfig, mut body: F) -> Vec<Metric> {
    let timing = time_fn(config, &mut body);
    let mut metrics = vec![timing];
    if let Some(allocs) = count_allocations(&mut body) {
        metrics.push(Metric::deterministic(
            ALLOCATIONS,
            allocs as f64,
            "count",
            true,
        ));
    }
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_fn_produces_requested_sample_count() {
        let config = TimingConfig {
            warmup_iters: 1,
            samples: 8,
            batch: 4,
        };
        let metric = time_fn(config, || {
            std::hint::black_box(1 + 1);
        });
        assert_eq!(metric.name, WALL_CLOCK);
        assert_eq!(metric.unit, "ns");
        assert!(metric.lower_is_better);
        assert_eq!(metric.samples.as_ref().map(Vec::len), Some(8));
    }

    #[test]
    fn count_allocations_returns_none_without_shim() {
        // The test binary does not install CountingAllocator as the global
        // allocator, so the probe must report the shim absent rather than zero.
        assert_eq!(count_allocations(|| {}), None);
    }
}
