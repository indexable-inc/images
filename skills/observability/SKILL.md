---
name: observability
description: >
  Instrument Rust services and CLIs on the ix repo: structured tracing spans, process-wide
  counters, `/debug` and `/health` endpoints, and the Flight recorder (tracing + GlobalAlloc
  → Parquet, queryable with DuckDB). Also the escalation ladder for "who's allocating?" —
  dhat, jemalloc-pprof, tracing-allocator, samply, tracy — and the anti-patterns to avoid
  (println ladders, hand-parsed /proc, unmeasured perf claims). Loads whenever you're adding
  logging/tracing, picking a profiler, debugging a perf or memory regression, or asking where
  allocations are coming from.
---

# Observability

Observability is cheap when added first, expensive when added late. Default to **instrumenting heavily**. Every long-running service / non-trivial CLI ships with:

1. **Structured tracing spans.** `#[tracing::instrument(level = "debug", skip_all)]` on every function with non-trivial work. `info` for RPC handlers, lifecycle events (VM start/stop/snapshot), public boundaries. `debug` for per-derivation / per-request internals. `trace` for tight inner loops. Filter at runtime with `RUST_LOG=pkg::mod=debug`. ¬ `println!` / `eprintln!` debug ladders — you lose them at compile time and can't toggle them in prod.
2. **Process-wide counters** at every suspected hot path. `AtomicU64::fetch_add(1, Relaxed)` is ~1 ns; keep them on in release. Publish via a `snapshot() -> &[(&'static str, u64)]` fn. Don't gate behind `cfg(debug_assertions)` — you want the same counters in a prod incident as in local debugging.
3. **A `/debug` or `/health` endpoint** on any service. Include `procfs::process::Process::myself()?.status()?` (VmRSS/VmPeak/VmSize), counter snapshot, queue depths, uptime, build info. Typed via `procfs`, ¬ hand-parsed `/proc/self/status`.
4. **Flight recorder** (the repo-native tier-2/3 combo) on any binary you want to debug perf or memory on. `crates/flight/` streams tracing spans + every `GlobalAlloc` event into a per-thread lock-free ring, drains rotated Parquet files. Query the result with DuckDB.

## Flight — the one recorder you should reach for first

Three crates, all under `crates/flight/`:

- `flight` — core (ring, drain, writer, schema, span thread-local, name intern, `init` + `Guard`)
- `flight-trace` — `tracing_subscriber::Layer` (`flight_trace::layer()`)
- `flight-alloc` — `GlobalAlloc` wrapper (`flight_alloc::FlightAlloc`, inner defaults to mimalloc)

Wire it like this at binary entry:

```rust
#[global_allocator]
static A: flight_alloc::FlightAlloc = flight_alloc::FlightAlloc::new();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = flight::init(flight::config::Config {
        out_dir: "/tmp/flight".into(),
        rotate_rows: core::num::NonZeroU64::new(1_000_000).unwrap(),
        rotate_bytes: core::num::NonZeroU64::new(128 << 20).unwrap(),
        ring_capacity: core::num::NonZeroU32::new(1 << 16).unwrap(),
        drain_interval: core::time::Duration::from_millis(50),
    })?;
    tracing_subscriber::registry()
        .with(flight_trace::layer())
        .init();
    // ... app ...
    Ok(())
}
```

Then query the resulting Parquet with DuckDB:

```sh
# Top allocation sites by total bytes, grouped by span name.
duckdb -c "
  SELECT n.text, COUNT(*), SUM(e.size)
  FROM read_parquet('/tmp/flight/flight-*.parquet') e
  LEFT JOIN read_parquet('/tmp/flight/names-*.parquet') n USING (name_ptr)
  WHERE e.kind = 0         -- alloc
  GROUP BY n.text ORDER BY SUM(e.size) DESC LIMIT 50"

# Retained bytes per span (alloc - dealloc, still live at process exit).
duckdb -c "
  SELECT n.text,
         SUM(CASE WHEN e.kind = 0 THEN e.size ELSE 0 END)
       - SUM(CASE WHEN e.kind = 1 THEN e.size ELSE 0 END) AS retained
  FROM read_parquet('/tmp/flight/flight-*.parquet') e
  LEFT JOIN read_parquet('/tmp/flight/names-*.parquet') n USING (name_ptr)
  GROUP BY n.text HAVING retained > 0 ORDER BY retained DESC LIMIT 50"

# Allocation timeline for one span, bucketed 100ms.
duckdb -c "
  SELECT floor(e.ts_nanos / 1e8) * 0.1 AS t_s, SUM(e.size)
  FROM read_parquet('/tmp/flight/flight-*.parquet') e
  LEFT JOIN read_parquet('/tmp/flight/names-*.parquet') n USING (name_ptr)
  WHERE e.kind = 0 AND n.text = 'my_hot_span'
  GROUP BY t_s ORDER BY t_s"
```

Flight replaces what dhat, jemalloc-pprof, tracing-allocator, and a hand-rolled CSV sampler would each give you individually — in one queryable table. **Reach for flight before those external tools.** The external tiers still exist for their niches but flight is the default.

## Escalation tiers for "who's allocating?" — don't skip levels

| Tier | Tool | Overhead | When |
|------|------|----------|------|
| 1 | budget gates on rung / request | ~0 | detect regression |
| 2 | in-process counters + snapshot | ~1 ns/op | narrow to a code path |
| 3 | heap profiler — `dhat-rs` | 3–5× | exact callsite, don't know where to look |
| 3′ | heap profiler — `tikv-jemallocator` + `jemalloc-pprof` | 10–20% | prod-safe heap profile (sampled backtraces) |
| 3″ | `tracing-allocator` (attribute to tracing spans) | 5–10% | comparing already-spanned hot paths |
| 4 | CPU / sampling profiler — `samply` / `perf` | ~0 (sampled) | hot-function work, not alloc |
| 5 | full tracing — `tracy` / `perfetto` | moderate | causal timeline across threads/processes |

Pick between 3 / 3′ / 3″:

- **Don't know which function → `dhat-rs`.** Full backtrace per alloc, zero prep work. Treemap viewer ranks callsites by retained bytes. Slow — keep runs to 10–30 s.
- **Prod / long runs → `jemalloc-pprof`.** Sampled, much cheaper, emits pprof flamegraphs (`jeprof`). Requires swapping to jemalloc.
- **Already span-annotated and want to compare N candidates → `tracing-allocator`.** Attributes to current span; only as granular as your spans.

**Gating heap profilers**: always behind a cargo feature, never default on.

```toml
[features]
heap-profile = ["dep:dhat"]
# or
heap-profile-jemalloc = ["dep:tikv-jemallocator", "dep:jemalloc-pprof"]
```

```rust
#[cfg(feature = "heap-profile")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    #[cfg(feature = "heap-profile")]
    let _profiler = dhat::Profiler::new_heap();
    // ... rest of main ...
}
// build:  cargo build --release --features heap-profile
// run:    ./target/.../binary <args>
// view:   open dhat-heap.json in dhat_viewer
```

## Anti-patterns

- `println!("here {:?}", x)` added and removed across a debugging session. Use `tracing::debug!` / `tracing::trace!` and `RUST_LOG` — the logs stay, emission toggles.
- `#[cfg(debug_assertions)] let t = Instant::now();` around a function. Either it's worth measuring in release too (use a counter or span), or it's not worth measuring.
- Per-thread backtrace capture on the hot path in release. Tier 3+ is the right place for that — behind a feature flag.
- Hand-parsing `/proc`. Use `procfs` (typed wrappers).
- "Just check with `ps`" or manual `htop` watching. You can't replay it, compare runs, or put it in CI. Wire the CSV sampler or flight.
- Unmeasured perf claims in PR descriptions. Attach a baseline CSV diff or a flamegraph.

## Related skill

- `hillclimb` — how these tiers integrate with a ladder + budget-gated workflow.
