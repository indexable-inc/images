mod badge;
mod filter;

use std::{io::Write, path::PathBuf};

use clap::Parser;
use clone_detect::{DetectConfig, DetectionResult, instances};
use clone_scanner::{Config, Scanner};
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
}

const DEFAULT_MIN_LINES: usize = 5;
const DEFAULT_MIN_NODES: usize = 10;
const DEFAULT_THRESHOLD: f64 = 0.7;
const DEFAULT_WINDOW_SIZE: usize = 3;

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
        Ok(has_clones) => {
            if has_clones {
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
        result = filter::by_patterns(result, &patterns)?;
    }

    let has_clones = !result.instances.is_empty();

    output_json(&result, config.pretty)?;

    if let Some(badge_path) = &args.badge {
        badge::write(badge_path, result.stats.duplication_pct)?;
    }

    Ok(has_clones)
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
    });

    let mut ignore = file.ignore.unwrap_or_default();
    ignore.extend(args.ignore.iter().cloned());

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
    }
}

fn output_json(result: &DetectionResult, pretty: bool) -> Result<(), RunError> {
    let mut stdout = std::io::stdout().lock();
    if pretty {
        serde_json::to_writer_pretty(&mut stdout, result).context(JsonWriteSnafu)?;
    } else {
        serde_json::to_writer(&mut stdout, result).context(JsonWriteSnafu)?;
    }
    writeln!(stdout).context(StdoutWriteSnafu)?;
    Ok(())
}
