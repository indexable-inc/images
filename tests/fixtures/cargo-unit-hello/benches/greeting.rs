use tango_bench::{IntoBenchmarks, benchmark_fn, tango_benchmarks, tango_main};

fn benchmarks() -> impl IntoBenchmarks {
    [benchmark_fn("greeting", |b| {
        b.iter(cargo_unit_hello::greeting)
    })]
}

tango_benchmarks!(benchmarks());
tango_main!();
