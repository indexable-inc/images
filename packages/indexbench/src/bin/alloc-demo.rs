//! A tiny deterministic allocation bench, the worked example for the
//! reproducible flake-check path.
//!
//! It installs `indexbench`'s [`CountingAllocator`](indexbench::micro::CountingAllocator)
//! as the global allocator, performs an *exactly known* number of heap
//! allocations, and prints an `@bench name=allocations …` line. The macro
//! harness records that as a deterministic metric, and the
//! `indexbench-self-demo-alloc` flake check asserts it stays within a budget —
//! a gate timing and RSS cannot be (they are not reproducible in the sandbox).
//!
//! The workload is `EXPECTED_ALLOCATIONS` plain `Box::new` calls rather than a
//! `Vec<String>` of `format!`ed items: a `Box::new` is exactly one allocation
//! with no realloc and no hidden std buffer, so the count is a fixed constant
//! across toolchains. The flake-check budget (`lib/per-system.nix`) is that same
//! constant, so an unrelated `nixpkgs`/`rustc` bump never reddens the gate; only
//! an actual added allocation does.
//!
//! This is intentionally a standalone binary rather than a library function: a
//! counting `#[global_allocator]` is process-global, so it must live in the
//! binary crate root that actually runs under measurement.

use indexbench::micro::{CountingAllocator, count_allocations};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

/// Heap allocations the workload makes, by construction. Kept in sync with the
/// `budgets.allocations` value the flake check asserts in `lib/per-system.nix`.
const EXPECTED_ALLOCATIONS: u64 = 64;

fn main() {
    // Exactly `EXPECTED_ALLOCATIONS` allocations: one `Box` per iteration, each
    // forced to materialize with `black_box` so the optimizer cannot elide it,
    // and dropped (a dealloc, which the counter does not tally). No realloc, no
    // implicit std buffer — the count is identical on every platform and build.
    let Some(count) = count_allocations(|| {
        for index in 0..EXPECTED_ALLOCATIONS {
            let boxed = std::hint::black_box(Box::new(index));
            drop(std::hint::black_box(boxed));
        }
    }) else {
        // The counting allocator is not installed, so the count would be a
        // misleading zero. Fail loudly rather than emit `value=0`, which would
        // silently satisfy any budget and turn the gate into a no-op.
        eprintln!(
            "indexbench-alloc-demo: CountingAllocator not installed; cannot measure allocations"
        );
        std::process::exit(1);
    };

    // The macro harness ingests this line as a deterministic metric.
    println!("@bench name=allocations value={count} unit=count lower_is_better=true");
}
