//! A tiny deterministic allocation bench, used as the worked example for the
//! reproducible flake-check path.
//!
//! It installs `indexbench`'s [`CountingAllocator`](indexbench::micro::CountingAllocator)
//! as the global allocator, performs a fixed amount of allocation, and prints an
//! `@bench name=allocations …` line. Because the allocation count is identical
//! on every run, the macro harness records the same deterministic metric each
//! time, which makes a `nix flake check` that gates on it reproducible — a
//! property timing and RSS do not have.
//!
//! This is intentionally a standalone binary rather than a function in the
//! library: a counting `#[global_allocator]` is process-global, so it must live
//! in the binary crate root that actually runs under measurement.

use indexbench::micro::{CountingAllocator, count_allocations};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn main() {
    // A fixed workload: build a vector of owned strings. The exact count is an
    // implementation detail; what matters is that it is identical run to run.
    let count = count_allocations(|| {
        let mut bucket: Vec<String> = Vec::new();
        for index in 0..32 {
            bucket.push(format!("item-{index}"));
        }
        std::hint::black_box(&bucket);
    })
    .unwrap_or(0);

    // The macro harness ingests this line as a deterministic metric.
    println!("@bench name=allocations value={count} unit=count lower_is_better=true");
}
