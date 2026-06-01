//! Declaring benches: [`BenchSuite`], [`MicroBench`], and [`MacroBench`].
//!
//! A consumer builds a [`BenchSuite`], registers micro benches (a Rust closure)
//! and macro benches (an external command), and hands the suite to the
//! [runner](crate::run) which executes each one into a [`Run`](crate::schema::Run)
//! and records it. The Nix `lib.mkBenchSuite` helper produces the wrapper that
//! calls this same surface, so a suite declared in Nix and one declared in Rust
//! go through one code path.

use crate::micro::TimingConfig;

/// A Rust closure benchmarked in-process.
///
/// The closure is `FnMut` so it can carry per-call state. `config` controls the
/// timed-loop sampler; defaulting it gives the standard warm-up/sample/batch
/// shape.
pub struct MicroBench<'a> {
    /// Bench name, unique within the suite.
    pub name: String,
    /// Sampler tuning.
    pub config: TimingConfig,
    /// The closure under test.
    pub body: Box<dyn FnMut() + 'a>,
}

impl<'a> MicroBench<'a> {
    /// Declare a micro bench with the default [`TimingConfig`].
    pub fn new(name: impl Into<String>, body: impl FnMut() + 'a) -> Self {
        Self {
            name: name.into(),
            config: TimingConfig::default(),
            body: Box::new(body),
        }
    }

    /// Override the sampler tuning.
    #[must_use]
    pub const fn with_config(mut self, config: TimingConfig) -> Self {
        self.config = config;
        self
    }
}

/// An external command benchmarked out-of-process.
///
/// The command is run `runs` times; the harness records `wall_clock` and
/// `max_rss` plus any `@bench` custom metrics it prints.
#[derive(Debug, Clone)]
pub struct MacroBench {
    /// Bench name, unique within the suite.
    pub name: String,
    /// Program to execute.
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
    /// How many times to run it.
    pub runs: u32,
}

impl MacroBench {
    /// Declare a macro bench. `runs` defaults to
    /// [`DEFAULT_MACRO_RUNS`](crate::compare::DEFAULT_MACRO_RUNS), which clears
    /// the comparator's `MIN_SAMPLES` floor so the built-in timing/RSS metrics
    /// land in the distributional regime by default.
    pub fn new(name: impl Into<String>, program: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            runs: crate::compare::DEFAULT_MACRO_RUNS,
        }
    }

    /// Override the run count.
    #[must_use]
    pub const fn with_runs(mut self, runs: u32) -> Self {
        self.runs = runs;
        self
    }
}

/// A named group of micro and macro benches.
///
/// The suite name becomes the `suite` field on every [`Run`](crate::schema::Run)
/// it produces, and is the namespace the comparator and store key on.
#[derive(Default)]
pub struct BenchSuite<'a> {
    /// The suite name.
    pub name: String,
    /// In-process Rust benches.
    pub micro: Vec<MicroBench<'a>>,
    /// Out-of-process command benches.
    pub macros: Vec<MacroBench>,
}

impl<'a> BenchSuite<'a> {
    /// Create an empty suite.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            micro: Vec::new(),
            macros: Vec::new(),
        }
    }

    /// Register a micro bench, returning the suite for chaining.
    #[must_use]
    pub fn micro(mut self, bench: MicroBench<'a>) -> Self {
        self.micro.push(bench);
        self
    }

    /// Register a macro bench, returning the suite for chaining.
    #[must_use]
    pub fn macro_bench(mut self, bench: MacroBench) -> Self {
        self.macros.push(bench);
        self
    }
}
