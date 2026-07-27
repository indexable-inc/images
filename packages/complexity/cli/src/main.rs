//! Rank the units of a tree by how hard they are to hold in your head, and
//! gate the count of the worst ones so it can only go down.
//!
//! The output is JSON on stdout and a human summary on stderr, matching the
//! sibling `clone` tool so both can be piped the same way.

mod config;
mod quantile;
mod report;

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::ExitCode,
};

use ast_merge_langs::{Lang, detect};
use clap::Parser as _;
use rayon::prelude::*;
use snafu::{ResultExt as _, Snafu};

use report::{Located, Report, Stats};

/// How many units the report lists by default. The literature on ranking
/// tools is blunt about this: a list nobody reads to the end is a list that
/// does nothing, so precision at the top beats recall over the whole tree.
const DEFAULT_TOP: usize = 25;

#[derive(clap::Parser, Debug)]
#[command(name = "complexity", version, about)]
struct Args {
    /// Directory or file to measure.
    #[arg(default_value = ".")]
    path: PathBuf,
    /// Print the size-weighted threshold quantiles per language instead of a
    /// ranking. These are the numbers that belong in `complexity.toml`.
    #[arg(long)]
    quantiles: bool,
    /// How many units to list. Every unit is still counted in `stats`.
    #[arg(long)]
    top: Option<usize>,
    /// Fail when more than this many units are at or above their language's
    /// threshold. Overrides `[budget] max_over_threshold`.
    #[arg(long)]
    max_over_threshold: Option<usize>,
    /// Extra ignore glob, repeatable. Merged with the config's list.
    #[arg(long, action = clap::ArgAction::Append)]
    ignore: Vec<String>,
    /// Indent the JSON.
    #[arg(long)]
    pretty: bool,
}

#[derive(Debug, Snafu)]
enum RunError {
    #[snafu(display("failed to load {}", config::FILENAME))]
    Config { source: config::Error },
    #[snafu(display("failed to walk {path}"))]
    Walk {
        path: String,
        source: repo_walker::WalkError,
    },
    #[snafu(display("{pattern} is not a valid glob"))]
    BadGlob {
        pattern: String,
        source: glob::PatternError,
    },
    #[snafu(display("failed to read {path}"))]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[snafu(display("failed to parse {path}"))]
    Parse {
        path: String,
        source: ast_merge_ast::Error,
    },
    #[snafu(display("failed to serialize the report"))]
    Json { source: serde_json::Error },
    #[snafu(display("failed to write the report"))]
    Stdout { source: std::io::Error },
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            tracing::error!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Returns whether the gate passed.
fn run() -> Result<bool, RunError> {
    let args = Args::parse();
    let config = config::load(&args.path).context(ConfigSnafu)?;

    let patterns = compile_globs(&config.ignore, &args.ignore)?;
    let paths = discover(&args.path, &patterns)?;
    let measured = measure_all(&paths)?;

    if args.quantiles {
        report_quantiles(&measured);
        return Ok(true);
    }

    let mut units: Vec<Located> = measured
        .iter()
        .flat_map(|file| {
            let threshold = config.threshold.get(file.language.profile().name).copied();
            file.units.iter().map(move |unit| Located {
                file: file.path.clone(),
                language: file.language.profile().name.to_owned(),
                over_threshold: threshold.is_some_and(|limit| unit.cognitive >= limit),
                unit: unit.clone(),
            })
        })
        .collect();

    // Descending by the headline metric, then by size, then by path so the
    // order is stable across runs and a diff of two reports is readable.
    units.sort_by(|left, right| {
        right
            .unit
            .cognitive
            .cmp(&left.unit.cognitive)
            .then(right.unit.lines.cmp(&left.unit.lines))
            .then(left.file.cmp(&right.file))
            .then(left.unit.start_line.cmp(&right.unit.start_line))
    });

    let stats = Stats {
        files_scanned: paths.len(),
        files_measured: measured.len(),
        units: units.len(),
        over_threshold: units.iter().filter(|unit| unit.over_threshold).count(),
        total_cognitive: units
            .iter()
            .map(|unit| u64::from(unit.unit.cognitive))
            .sum(),
    };

    let budget = args
        .max_over_threshold
        .or(config.budget.max_over_threshold)
        .map(|budget| report::Gate {
            over_threshold: stats.over_threshold,
            budget,
            pass: report::passes(stats.over_threshold, budget),
        });

    summarize(&stats, budget.as_ref(), &units);

    let passed = budget.as_ref().is_none_or(|gate| gate.pass);
    units.truncate(args.top.unwrap_or(DEFAULT_TOP));
    write_json(
        &Report {
            units,
            stats,
            budget,
        },
        args.pretty,
    )?;
    Ok(passed)
}

fn compile_globs(config: &[String], extra: &[String]) -> Result<Vec<glob::Pattern>, RunError> {
    config
        .iter()
        .chain(extra)
        .map(|pattern| {
            glob::Pattern::new(pattern).context(BadGlobSnafu {
                pattern: pattern.clone(),
            })
        })
        .collect()
}

/// Files to measure, with the ignore globs already applied. Filtering here
/// rather than after measuring is what makes an ignore actually move the gated
/// number: an ignored file leaves the denominator too.
fn discover(root: &Path, patterns: &[glob::Pattern]) -> Result<Vec<PathBuf>, RunError> {
    if root.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    let scanner = repo_walker::FileScanner::new(root, repo_walker::WalkOptions::default());
    let mut paths = Vec::new();
    for entry in scanner {
        let path = entry.context(WalkSnafu {
            path: root.display().to_string(),
        })?;
        let ignored = patterns
            .iter()
            .any(|pattern| pattern.matches_path(path.as_path()));
        if !ignored && detect(&path).is_some() {
            paths.push(path);
        }
    }
    // Sorted so two runs over the same tree emit the same report.
    paths.sort();
    Ok(paths)
}

struct Measured {
    path: PathBuf,
    language: Lang,
    units: Vec<complexity_metric::Unit>,
}

/// Parse and score every file. Parsing dominates the wall clock, so it runs
/// across the pool; a file that fails to read or parse aborts the run rather
/// than silently lowering the count the gate reads.
fn measure_all(paths: &[PathBuf]) -> Result<Vec<Measured>, RunError> {
    paths
        .par_iter()
        .map(|path| {
            let Some(language) = detect(path) else {
                return Ok(None);
            };
            if complexity_metric::kinds::profile(language).is_none() {
                return Ok(None);
            }
            let display = path.display().to_string();
            let Ok(source) = std::fs::read(path)
                .context(ReadSnafu { path: &display })
                .map(String::from_utf8)?
            else {
                // Not UTF-8, so not source this tool can read.
                return Ok(None);
            };
            let parsed = ast_merge_ast::tree(&source, &language.to_tree_sitter())
                .context(ParseSnafu { path: display })?;
            Ok(Some(Measured {
                path: path.clone(),
                language,
                units: complexity_metric::measure(&parsed.tree, language),
            }))
        })
        .filter_map(Result::transpose)
        .collect()
}

fn report_quantiles(measured: &[Measured]) {
    let mut by_language: std::collections::BTreeMap<&str, Vec<(u32, usize)>> =
        std::collections::BTreeMap::new();
    for file in measured {
        let entry = by_language.entry(file.language.profile().name).or_default();
        entry.extend(file.units.iter().map(|unit| (unit.cognitive, unit.lines)));
    }

    println!("# Size-weighted cognitive-complexity quantiles, per language.");
    println!("# pN is the value at which the worst (100-N)% of this repo by volume begins.");
    println!("[threshold]");
    for (language, samples) in &mut by_language {
        let bands: Vec<String> = quantile::COVERAGE
            .iter()
            .map(|(label, coverage)| {
                let value = quantile::threshold(samples, *coverage);
                format!("{label}={}", value.unwrap_or_default())
            })
            .collect();
        let p90 = quantile::threshold(samples, 0.90).unwrap_or_default();
        println!(
            "{language} = {p90}  # {} units, {}",
            samples.len(),
            bands.join(" ")
        );
    }
}

fn summarize(stats: &Stats, budget: Option<&report::Gate>, units: &[Located]) {
    tracing::info!(
        files = stats.files_measured,
        units = stats.units,
        over_threshold = stats.over_threshold,
        "measured",
    );
    for unit in units.iter().take(5) {
        tracing::info!(
            file = %unit.file.display(),
            line = unit.unit.start_line,
            cognitive = unit.unit.cognitive,
            nesting = unit.unit.nesting,
            lines = unit.unit.lines,
            "{}",
            unit.unit.signature,
        );
    }
    if let Some(gate) = budget {
        if gate.pass {
            tracing::info!(
                over_threshold = gate.over_threshold,
                budget = gate.budget,
                "complexity budget ok",
            );
        } else {
            tracing::error!(
                over_threshold = gate.over_threshold,
                budget = gate.budget,
                "complexity budget exceeded: break down a unit above, or state why the budget moves",
            );
        }
    }
}

fn write_json(report: &Report, pretty: bool) -> Result<(), RunError> {
    let text = if pretty {
        serde_json::to_string_pretty(report)
    } else {
        serde_json::to_string(report)
    }
    .context(JsonSnafu)?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{text}").context(StdoutSnafu)
}
