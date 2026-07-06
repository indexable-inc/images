mod badge;
mod diff;
mod filter;
mod gate;

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use clap::Parser;
use clone_detect::{DetectConfig, DetectionResult, instances};
use clone_scanner::{Config, Scanner};
use gate::{DiffGate, GateReport, GlobalGate};
use serde_json::Value;
use snafu::ResultExt as _;

/// Config file name discovered by walking up from the scan target directory.
const CONFIG_FILENAME: &str = "clone.toml";

#[derive(Parser, Debug)]
#[command(name = "clone", version, about)]
struct Args {
    #[arg(default_value = ".")]
    path: PathBuf,

    #[arg(long)]
    type3: bool,

    #[arg(long)]
    threshold: Option<f64>,

    #[arg(long)]
    min_lines: Option<usize>,

    #[arg(long)]
    min_nodes: Option<usize>,

    #[arg(long)]
    pretty: bool,

    #[arg(long, action = clap::ArgAction::Append)]
    ignore: Vec<String>,

    /// Enable statement-sequence clone detection
    #[arg(long)]
    sequences: bool,

    /// Sliding window size for sequence detection
    #[arg(long)]
    window_size: Option<usize>,

    /// Write an SVG duplication badge to this path
    #[arg(long)]
    badge: Option<PathBuf>,

    /// Fail if whole-scan `duplication_pct` exceeds this percentage. Overrides
    /// `[budget] global_pct`.
    #[arg(long)]
    max_global_pct: Option<f64>,

    /// Enable the diff gate against a git base rev (default resolution:
    /// `[budget] diff_base`, else `origin/main`). Fails if duplication over the
    /// lines changed since the merge base exceeds `--max-diff-pct`.
    // `Option<Option<T>>` is clap's idiom for an optional flag with an optional
    // value: outer `None` = flag absent (gate off), `Some(None)` = `--diff` with
    // no base (resolve from config/default), `Some(Some(b))` = `--diff b`.
    #[expect(
        clippy::option_option,
        reason = "clap encodes flag-present-vs-value-present as Option<Option<_>>"
    )]
    #[arg(long, value_name = "BASE")]
    diff: Option<Option<String>>,

    /// Fail if duplication over changed lines exceeds this percentage.
    /// Overrides `[budget] diff_pct`. Only meaningful with `--diff`.
    #[arg(long)]
    max_diff_pct: Option<f64>,
}

/// Project-level configuration loaded from `clone.toml`.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    min_lines: Option<usize>,
    min_nodes: Option<usize>,
    threshold: Option<f64>,
    type3: Option<bool>,
    sequences: Option<bool>,
    window_size: Option<usize>,
    pretty: Option<bool>,
    ignore: Option<Vec<String>>,
    budget: Option<BudgetConfig>,
}

/// The `[budget]` table: duplication ceilings for the gates.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetConfig {
    /// Ceiling for whole-scan `duplication_pct`.
    global_pct: Option<f64>,
    /// Ceiling for duplication over changed lines (diff gate).
    diff_pct: Option<f64>,
    /// Default base rev for the diff gate (e.g. `origin/main`).
    diff_base: Option<String>,
}

/// Resolved configuration after merging CLI flags over file config over defaults.
struct ResolvedConfig {
    min_lines: usize,
    min_nodes: usize,
    threshold: f64,
    type3: bool,
    sequences: bool,
    window_size: usize,
    pretty: bool,
    ignore: Vec<String>,
    /// Whole-scan `duplication_pct` ceiling. Defaults to `0.0`, which reproduces
    /// the legacy "any surviving clone fails" behavior when no budget is set.
    global_pct: f64,
    /// The resolved diff gate, or `None` when `--diff` was not passed.
    diff: Option<DiffGateConfig>,
}

/// Diff gate parameters, present only when the gate is enabled.
struct DiffGateConfig {
    base: String,
    budget_pct: f64,
}

const DEFAULT_MIN_LINES: usize = 5;
const DEFAULT_MIN_NODES: usize = 10;
const DEFAULT_THRESHOLD: f64 = 0.7;
const DEFAULT_WINDOW_SIZE: usize = 3;
/// Legacy behavior: with no budget configured, any surviving clone fails the
/// global gate, i.e. a ceiling of zero duplication.
const DEFAULT_GLOBAL_PCT: f64 = 0.0;
/// Diff gate default ceiling: no duplication allowed on changed lines.
const DEFAULT_DIFF_PCT: f64 = 0.0;
/// Diff gate default base rev when neither the flag nor config specifies one.
const DEFAULT_DIFF_BASE: &str = "origin/main";

#[derive(Debug, snafu::Snafu)]
enum RunError {
    #[snafu(display("scan failed"))]
    Scan { source: clone_scanner::Error },

    #[snafu(display("invalid glob pattern: {pattern}"))]
    BadGlob {
        pattern: String,
        source: glob::PatternError,
    },

    #[snafu(display("failed to write JSON output"))]
    JsonWrite { source: serde_json::Error },

    #[snafu(display("failed to write to stdout"))]
    StdoutWrite { source: std::io::Error },

    #[snafu(display("failed to write badge to {}", path.display()))]
    BadgeWrite {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("failed to read config file at {}", path.display()))]
    ConfigRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("failed to parse config file at {}", path.display()))]
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[snafu(display("path is not valid UTF-8: {}", path.display()))]
    NonUtf8Path { path: PathBuf },

    #[snafu(display("diff gate could not read git history"))]
    Diff { source: diff::DiffError },
}

/// Install a tracing subscriber reading filter directives from `RUST_LOG`
/// (defaulting to `info`) and writing formatted events to stderr. Replaces the
/// `ix` platform's `service_init::init`, which this standalone tool does not
/// depend on.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();
}

#[expect(
    clippy::print_stderr,
    reason = "CLI error output before/after tracing init"
)]
fn main() -> std::process::ExitCode {
    init_tracing();

    match run() {
        Ok(gate_failed) => {
            if gate_failed {
                std::process::ExitCode::FAILURE
            } else {
                std::process::ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("Error: {e:?}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool, RunError> {
    let args = Args::parse();

    let file_config = find_config(&args.path)?;
    let config = resolve_config(&args, file_config);

    let scan_config = Config {
        min_lines: config.min_lines,
        min_nodes: config.min_nodes,
        ..Config::default()
    };

    let scanner = Scanner::new(scan_config);
    let scan = scanner.directory(&args.path).context(ScanSnafu)?;

    let detect_config = DetectConfig {
        enable_type3: config.type3,
        type3_threshold: config.threshold,
        enable_sequences: config.sequences,
        sequence_window_size: config.window_size,
    };

    let mut result = instances(&scan, &detect_config);

    if !config.ignore.is_empty() {
        let patterns: Vec<glob::Pattern> = config
            .ignore
            .iter()
            .map(|p| glob::Pattern::new(p).context(BadGlobSnafu { pattern: p.clone() }))
            .collect::<Result<_, _>>()?;
        result = filter::by_patterns(result, &scan, &patterns)?;
    }

    let report = evaluate_gates(&result, &config, &args.path)?;

    output_json(&result, &report, config.pretty)?;

    if let Some(badge_path) = &args.badge {
        badge::write(badge_path, result.stats.duplication_pct)?;
    }

    // The run "fails" (exit FAILURE) iff any enabled gate failed.
    Ok(!report.passed())
}

/// Evaluate the enabled gates and emit a one-line human summary per gate to
/// stderr via tracing.
fn evaluate_gates(
    result: &DetectionResult,
    config: &ResolvedConfig,
    scan_path: &std::path::Path,
) -> Result<GateReport, RunError> {
    let global = GlobalGate::evaluate(result, config.global_pct);
    log_gate(
        "global",
        global.pass,
        global.duplication_pct,
        global.budget_pct,
    );

    // Git runs relative to the scanned tree: a file target anchors on its parent
    // directory, a directory target on itself. This is what makes `--diff` work
    // when the scan path is not the process's working directory.
    let repo_dir: PathBuf = if scan_path.is_file() {
        scan_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    } else {
        scan_path.to_path_buf()
    };

    let diff = config
        .diff
        .as_ref()
        .map(|cfg| {
            let diff = diff::changed_lines(&repo_dir, &cfg.base).context(DiffSnafu)?;
            let gate = DiffGate::evaluate(
                result,
                &diff.changed,
                cfg.budget_pct,
                cfg.base.clone(),
                diff.base_sha,
            );
            log_diff_gate(&gate);
            Ok(gate)
        })
        .transpose()?;

    Ok(GateReport {
        global: Some(global),
        diff,
    })
}

/// One-line pass/fail summary for a simple metric-vs-budget gate.
fn log_gate(name: &str, pass: bool, metric: f64, budget: f64) {
    let verdict = if pass { "PASS" } else { "FAIL" };
    tracing::info!(gate = name, %verdict, metric_pct = metric, budget_pct = budget, "clone {name} gate {verdict}: {metric:.4}% <= {budget:.4}%? {pass}");
}

/// One-line summary for the diff gate, including the resolved base and changed-
/// line counts.
fn log_diff_gate(gate: &DiffGate) {
    let verdict = if gate.pass { "PASS" } else { "FAIL" };
    tracing::info!(
        gate = "diff",
        %verdict,
        base = gate.base,
        base_sha = gate.base_sha,
        diff_pct = gate.diff_pct,
        budget_pct = gate.budget_pct,
        duplicated = gate.duplicated_changed_lines,
        changed = gate.changed_lines,
        "clone diff gate {verdict}: {dup}/{tot} changed lines duplicated = {pct:.4}% <= {budget:.4}%? {pass} (base {base}@{sha})",
        dup = gate.duplicated_changed_lines,
        tot = gate.changed_lines,
        pct = gate.diff_pct,
        budget = gate.budget_pct,
        pass = gate.pass,
        base = gate.base,
        sha = &gate.base_sha,
    );
}

/// Walk up from `start` looking for `clone.toml`. Returns `None` if not found.
fn find_config(start: &std::path::Path) -> Result<Option<FileConfig>, RunError> {
    let start = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());

    let mut dir = if start.is_file() {
        start.parent().map(std::path::Path::to_path_buf)
    } else {
        Some(start)
    };

    while let Some(d) = dir {
        let candidate = d.join(CONFIG_FILENAME);
        if candidate.is_file() {
            tracing::debug!(?candidate, "found clone config");
            let content = std::fs::read_to_string(&candidate)
                .context(ConfigReadSnafu { path: &candidate })?;
            let parsed: FileConfig =
                toml::from_str(&content).context(ConfigParseSnafu { path: &candidate })?;
            return Ok(Some(parsed));
        }
        dir = d.parent().map(std::path::Path::to_path_buf);
    }

    Ok(None)
}

/// Merge CLI args over file config over hardcoded defaults.
///
/// Priority: CLI flag > clone.toml > default.
/// Boolean flags use OR: if either CLI or config enables it, it's on.
/// Ignore lists are combined (config + CLI).
fn resolve_config(args: &Args, file: Option<FileConfig>) -> ResolvedConfig {
    let file = file.unwrap_or(FileConfig {
        min_lines: None,
        min_nodes: None,
        threshold: None,
        type3: None,
        sequences: None,
        window_size: None,
        pretty: None,
        ignore: None,
        budget: None,
    });

    let mut ignore = file.ignore.unwrap_or_default();
    ignore.extend(args.ignore.iter().cloned());

    let budget = file.budget.unwrap_or_default();

    // Global gate: CLI > `[budget] global_pct` > 0.0 (legacy any-clone-fails).
    let global_pct = args
        .max_global_pct
        .or(budget.global_pct)
        .unwrap_or(DEFAULT_GLOBAL_PCT);

    // Diff gate is opt-in at invocation. When enabled, the base is the flag's
    // argument, else `[budget] diff_base`, else `origin/main`; the budget is the
    // flag, else `[budget] diff_pct`, else 0.0.
    let diff = args.diff.as_ref().map(|flag_base| DiffGateConfig {
        base: flag_base
            .clone()
            .or_else(|| budget.diff_base.clone())
            .unwrap_or_else(|| DEFAULT_DIFF_BASE.to_owned()),
        budget_pct: args
            .max_diff_pct
            .or(budget.diff_pct)
            .unwrap_or(DEFAULT_DIFF_PCT),
    });

    ResolvedConfig {
        min_lines: args
            .min_lines
            .or(file.min_lines)
            .unwrap_or(DEFAULT_MIN_LINES),
        min_nodes: args
            .min_nodes
            .or(file.min_nodes)
            .unwrap_or(DEFAULT_MIN_NODES),
        threshold: args
            .threshold
            .or(file.threshold)
            .unwrap_or(DEFAULT_THRESHOLD),
        type3: args.type3 || file.type3.unwrap_or(false),
        sequences: args.sequences || file.sequences.unwrap_or(false),
        window_size: args
            .window_size
            .or(file.window_size)
            .unwrap_or(DEFAULT_WINDOW_SIZE),
        pretty: args.pretty || file.pretty.unwrap_or(false),
        ignore,
        global_pct,
        diff,
    }
}

/// Emit the detection result plus a `gate` object reporting each enabled gate.
/// The result and report serialize independently, then merge into one object so
/// the existing `instances`/`stats` schema is untouched and `gate` is additive.
fn output_json(
    result: &DetectionResult,
    report: &GateReport,
    pretty: bool,
) -> Result<(), RunError> {
    let value = serde_json::to_value(result).context(JsonWriteSnafu)?;
    let gate = serde_json::to_value(report).context(JsonWriteSnafu)?;

    // `DetectionResult` is a struct, so serde always yields a JSON object; a
    // non-object would mean the type changed shape, hence the explicit expect.
    let Value::Object(mut object) = value else {
        return Err(RunError::JsonWrite {
            source: <serde_json::Error as serde::ser::Error>::custom(
                "detection result did not serialize to an object",
            ),
        });
    };
    object.insert("gate".to_owned(), gate);
    let merged = Value::Object(object);

    let mut stdout = std::io::stdout().lock();
    if pretty {
        serde_json::to_writer_pretty(&mut stdout, &merged).context(JsonWriteSnafu)?;
    } else {
        serde_json::to_writer(&mut stdout, &merged).context(JsonWriteSnafu)?;
    }
    writeln!(stdout).context(StdoutWriteSnafu)?;
    Ok(())
}
