//! `search`: a daemon-free, content-addressed semantic code search.
//!
//! Every run rebuilds a local manifest (cheap; unchanged files are not
//! re-hashed), uploads only content the store is missing, waits for it to
//! embed, then searches. No daemon, no `--sync` flag: new files are picked up
//! and embedded automatically at search time. `--no-sync` skips that for a pure
//! offline search. Results are scoped to the current checkout via the manifest.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anstyle::{AnsiColor, Style};
use clap::{Args, Parser, Subcommand};
use indicatif::ProgressBar;
use search_core::{
    CodeScope, Config, DEFAULT_STORE, DisplayHit, Filter, FilterSpec, GrepOptions, GrepTargets,
    MixedbreadStore, Query, SearchOptions, Source, SourceAdapter, StoreStatus, build_filter,
    repo_slug,
};

/// How long to wait for freshly uploaded files to finish embedding.
const INDEX_TIMEOUT: Duration = Duration::from_mins(2);

/// Command-line arguments.
///
/// A bare invocation (`search <pattern> [path]`) runs a natural-language
/// semantic search, preserving the original flat interface. The `grep`
/// subcommand runs a regular expression over the same indexed chunks. Both
/// honor the shared connection flags (`--store`, `--base-url`).
#[derive(Debug, Parser)]
#[command(name = "search", about, version)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
struct Cli {
    /// Semantic search arguments for a bare invocation (no subcommand).
    #[command(flatten)]
    semantic: SemanticArgs,

    /// Run a regex grep instead of a semantic search.
    #[command(subcommand)]
    command: Option<Command>,
}

/// Subcommands. Absent means the bare search path runs.
#[derive(Debug, Subcommand)]
enum Command {
    /// Grep the indexed chunks with a regular expression.
    Grep(GrepArgs),
    /// Ingest a non-code source (slack, linear) from an export directory.
    Ingest(IngestArgs),
    /// Garbage-collect a non-code source: delete store records absent from the
    /// export (a full-snapshot reconcile, never a window slice).
    Gc(IngestArgs),
}

/// Arguments for `ingest` and `gc`. Code is indexed by an ordinary `search`, so
/// these cover the record sources only.
#[derive(Debug, Args)]
struct IngestArgs {
    /// Which source to ingest: slack or linear.
    source: String,

    /// Path to the export directory.
    dir: String,

    /// Store name (one store holds every source's content).
    #[arg(long, env = "MXBAI_STORE")]
    store: Option<String>,

    /// Mixedbread API base URL.
    #[arg(long = "base-url", env = "MXBAI_BASE_URL")]
    base_url: Option<String>,
}

/// Scope selectors shared by the semantic and grep paths. With no selector the
/// default is all sources, with code scoped to the current worktree.
#[derive(Debug, Args)]
struct ScopeArgs {
    /// Restrict to these sources (repeatable): code, slack, linear, web.
    #[arg(long = "source", value_name = "SOURCE")]
    sources: Vec<String>,

    /// Exclude these sources (repeatable).
    #[arg(long = "not-source", value_name = "SOURCE")]
    not_sources: Vec<String>,

    /// Restrict code to a repository slug (e.g. indexable-inc/index).
    #[arg(long)]
    repo: Option<String>,

    /// Search code across all repositories, not just this checkout.
    #[arg(long = "all-repos")]
    all_repos: bool,

    /// Search this repository across all worktrees, not just files checked out
    /// here.
    #[arg(long = "all-worktrees")]
    all_worktrees: bool,
}

/// Resolve scope selectors into a server-side metadata filter and the code
/// scoping mode.
fn resolve_scope(scope: &ScopeArgs, root: &Path) -> anyhow::Result<(Option<Filter>, CodeScope)> {
    let sources = parse_sources(&scope.sources)?;
    let exclude_sources = parse_sources(&scope.not_sources)?;

    // A repo / all-repos / all-worktrees query is server-filtered: the manifest
    // can only answer "files checked out here", so anything coarser must trust
    // the metadata filter instead.
    let (repo, code_scope) = if scope.all_repos {
        (None, CodeScope::ServerFiltered)
    } else if let Some(repo) = scope.repo.clone() {
        (Some(repo), CodeScope::ServerFiltered)
    } else if scope.all_worktrees {
        (Some(repo_slug(root).as_str().to_owned()), CodeScope::ServerFiltered)
    } else {
        (None, CodeScope::WorktreeExact)
    };

    let spec = FilterSpec {
        sources,
        exclude_sources,
        repo,
    };
    Ok((build_filter(&spec), code_scope))
}

fn parse_sources(values: &[String]) -> anyhow::Result<Vec<Source>> {
    values
        .iter()
        // A source may arrive comma-joined (`--source code,slack`) or repeated.
        .flat_map(|value| value.split(','))
        .filter(|value| !value.is_empty())
        .map(|value| value.parse::<Source>().map_err(anyhow::Error::from))
        .collect()
}

/// Arguments for the default search path. Flags mirror `mgrep search`
/// where they overlap.
// A CLI naturally has many independent boolean flags; a state machine would
// obscure, not clarify, the argument surface.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Args)]
struct SemanticArgs {
    /// The query to search for. Required for a bare search; omitted when a
    /// subcommand (grep/ingest/gc) is used.
    pattern: Option<String>,

    /// Directory to search in (defaults to the current directory).
    path: Option<String>,

    /// Maximum number of results to return.
    #[arg(short = 'm', long = "max-count", default_value_t = 10)]
    max_count: usize,

    /// Show the matched content under each result.
    #[arg(short = 'c', long)]
    content: bool,

    /// Synthesize an answer from the results instead of listing them.
    #[arg(short = 'a', long)]
    answer: bool,

    /// Search the store as-is: skip detecting and embedding new files.
    #[arg(long = "no-sync")]
    no_sync: bool,

    /// Disable result reranking (on by default).
    #[arg(long = "no-rerank")]
    no_rerank: bool,

    /// Include web results from the hosted web store.
    #[arg(short = 'w', long)]
    web: bool,

    /// Let the backend plan and run multiple searches.
    #[arg(long)]
    agentic: bool,

    /// Emit results as a JSON array on stdout instead of the human listing.
    /// Each element is `{path, source, start_line, num_lines, score, text}`.
    #[arg(long)]
    json: bool,

    /// Source and repo scope selectors.
    #[command(flatten)]
    scope: ScopeArgs,

    /// Store name (one store holds every worktree's content).
    #[arg(long, env = "MXBAI_STORE")]
    store: Option<String>,

    /// Mixedbread API base URL.
    #[arg(long = "base-url", env = "MXBAI_BASE_URL")]
    base_url: Option<String>,
}

/// Arguments for the `grep` subcommand. Grep is local-corpus only (no web
/// store) and shares the connection flags with the semantic path.
// Like `SemanticArgs`, this is a flat surface of independent boolean flags; a
// state machine would obscure rather than clarify it.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Args)]
struct GrepArgs {
    /// The regular expression to match against the indexed chunks.
    pattern: String,

    /// Directory to search in (defaults to the current directory).
    path: Option<String>,

    /// Maximum number of results to return.
    #[arg(short = 'm', long = "max-count", default_value_t = 10)]
    max_count: usize,

    /// Show the matched content under each result.
    #[arg(short = 'c', long)]
    content: bool,

    /// Match the pattern case-sensitively (case-insensitive by default).
    #[arg(short = 's', long = "case-sensitive")]
    case_sensitive: bool,

    /// Search the store as-is: skip detecting and embedding new files.
    #[arg(long = "no-sync")]
    no_sync: bool,

    /// Emit results as a JSON array on stdout instead of the human listing.
    /// Each element is `{path, source, start_line, num_lines, score, text}`.
    #[arg(long)]
    json: bool,

    /// Source and repo scope selectors.
    #[command(flatten)]
    scope: ScopeArgs,

    /// Store name (one store holds every worktree's content).
    #[arg(long, env = "MXBAI_STORE")]
    store: Option<String>,

    /// Mixedbread API base URL.
    #[arg(long = "base-url", env = "MXBAI_BASE_URL")]
    base_url: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Grep(args)) => run_grep(args).await,
        Some(Command::Ingest(args)) => run_ingest(args, false).await,
        Some(Command::Gc(args)) => run_ingest(args, true).await,
        None => run(cli.semantic).await,
    }
}

/// Ingest (or, with `gc`, garbage-collect) a record source into the store.
async fn run_ingest(cli: IngestArgs, gc: bool) -> anyhow::Result<()> {
    let store_name = cli.store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base_url = cli
        .base_url
        .unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let store = MixedbreadStore::from_login(base_url).await?;
    let dir = Path::new(&cli.dir);
    let source: Source = cli.source.parse()?;

    match source {
        Source::Linear => {
            let adapter = linear_export::LinearExport::open(dir)?;
            run_one_source(&adapter, &store, &store_name, gc).await
        }
        Source::Slack => {
            let adapter = slack_export::SlackExport::open(dir)?;
            run_one_source(&adapter, &store, &store_name, gc).await
        }
        Source::Code | Source::Web => anyhow::bail!(
            "ingest covers record sources (slack, linear); code is indexed by a normal `search`"
        ),
    }
}

async fn run_one_source(
    adapter: &(impl SourceAdapter + Sync),
    store: &MixedbreadStore,
    store_name: &str,
    gc: bool,
) -> anyhow::Result<()> {
    if gc {
        let report = search_core::gc_documents(adapter, store, store_name).await?;
        println!(
            "gc {}: deleted {} stale records, kept {}",
            adapter.source(),
            report.deleted,
            report.kept
        );
        return Ok(());
    }

    let bar = std::io::stderr().is_terminal().then(ProgressBar::new_spinner);
    if let Some(bar) = &bar {
        bar.set_style(progress_style::bar("cyan"));
        bar.set_prefix("uploading records");
    }
    let on_progress = |done: usize, total: usize| {
        if let (Some(bar), true) = (&bar, total > 0) {
            bar.set_length(u64::try_from(total).unwrap_or(u64::MAX));
            bar.set_position(u64::try_from(done).unwrap_or(u64::MAX));
        }
    };
    let report =
        search_core::sync_documents(adapter, store, store_name, INDEX_TIMEOUT, on_progress).await?;
    if let Some(bar) = &bar {
        bar.finish_and_clear();
    }
    println!(
        "ingest {}: uploaded {}, skipped {}, total {}",
        adapter.source(),
        report.uploaded,
        report.skipped,
        report.total
    );
    Ok(())
}

async fn run(cli: SemanticArgs) -> anyhow::Result<()> {
    // `pattern` is optional at the clap layer so a subcommand can be given
    // without it; a bare search still requires one.
    let pattern = cli
        .pattern
        .clone()
        .ok_or_else(|| anyhow::anyhow!("a query is required: `search <pattern> [path]`"))?;
    let root = resolve_root(cli.path.as_deref())?;
    anyhow::ensure!(
        !at_or_above_home(&root),
        "refusing to index {} (it is at or above your home directory); run from a project directory",
        root.display(),
    );

    // Color is decided once on stdout, where results print. `anstream` folds in
    // TTY detection, `NO_COLOR`, and `CLICOLOR_FORCE`, so piped output and
    // `NO_COLOR=1` both yield a plain palette that emits no escape codes.
    let palette = Palette::for_stdout();
    // Pick the islands variant from the terminal background, but only when we
    // will actually render highlighted snippets (`-c` on a color TTY):
    // detection writes an OSC background query to the terminal, so there is no
    // reason to probe it for a plain result list or machine-readable output.
    let theme = if cli.content {
        detect_theme(palette.color)
    } else {
        code_highlight::Theme::default()
    };

    let config = Config::default();
    let store_name = cli.store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base_url = cli
        .base_url
        .unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let store = MixedbreadStore::from_login(base_url.clone()).await?;

    let (filter, code_scope) = resolve_scope(&cli.scope, &root)?;
    let query = Query {
        root: &root,
        store_name: &store_name,
        base_url: &base_url,
        text: &pattern,
        top_k: cli.max_count.max(1),
        options: SearchOptions {
            rerank: !cli.no_rerank,
            agentic: cli.agentic,
        },
        sync: !cli.no_sync,
        include_web: cli.web,
        filters: filter.as_ref(),
        code_scope,
        index_timeout: INDEX_TIMEOUT,
    };

    // Progress UI, terminal only: an animated spinner during the manifest +
    // store-listing phase, flipping to determinate upload then embedding bars.
    // Piped output stays clean (no bar).
    let progress = IndexProgress::new();

    if cli.answer {
        anyhow::ensure!(
            !cli.json,
            "--json is not supported with --answer; pass one or the other",
        );
        let view = search_core::index_and_answer(
            &store,
            &query,
            &config,
            |done, total| progress.on_upload(done, total),
            |status| progress.on_poll(status),
        )
        .await?;
        progress.finish();
        println!("{}", view.answer);
        if !view.sources.is_empty() {
            println!();
            for (index, hit) in view.sources.iter().enumerate() {
                println!("{index}: {}", render(hit, cli.content, &palette, &root, theme));
            }
        }
    } else {
        let hits = search_core::index_and_semantic(
            &store,
            &query,
            &config,
            |done, total| progress.on_upload(done, total),
            |status| progress.on_poll(status),
        )
        .await?;
        progress.finish();
        print_hits(&hits, cli.json, cli.content, &palette, &root, theme)?;
    }

    Ok(())
}

/// Terminal progress for the index-then-query flow. The phases before any
/// upload (building the manifest, listing what the store already holds) have no
/// known total, so the bar starts as an animated spinner instead of a frozen
/// `0/0`; it flips to a determinate "uploading" bar once new files start
/// uploading, then to an "embedding" bar while the store embeds them. A
/// non-terminal stderr (piped output) gets no bar at all.
struct IndexProgress {
    bar: Option<ProgressBar>,
    /// Files uploaded this run, captured so the embedding bar knows its length.
    embed_total: AtomicU64,
    /// Whether the spinner has already flipped to the determinate upload bar.
    uploading: AtomicBool,
    /// Whether the upload bar has already flipped to the embedding bar.
    embedding: AtomicBool,
}

impl IndexProgress {
    /// Start an animated spinner (terminal only) for the pre-upload phase, so a
    /// slow manifest build or store listing reads as working, not hung.
    fn new() -> Self {
        let bar = std::io::stderr().is_terminal().then(ProgressBar::new_spinner);
        if let Some(bar) = &bar {
            bar.set_style(progress_style::spinner());
            bar.set_prefix("indexing");
            bar.set_message("scanning files, checking store");
            bar.enable_steady_tick(Duration::from_millis(120));
        }
        Self {
            bar,
            embed_total: AtomicU64::new(0),
            uploading: AtomicBool::new(false),
            embedding: AtomicBool::new(false),
        }
    }

    /// Upload-phase callback: `(uploaded_so_far, total_to_upload)`. The first
    /// call with a real total flips the spinner to a determinate bar.
    fn on_upload(&self, done: usize, total: usize) {
        let (Some(bar), true) = (&self.bar, total > 0) else {
            return;
        };
        let total = u64::try_from(total).unwrap_or(u64::MAX);
        self.embed_total.store(total, Ordering::Relaxed);
        if !self.uploading.swap(true, Ordering::Relaxed) {
            bar.set_style(progress_style::bar("cyan"));
            bar.set_prefix("uploading files");
        }
        bar.set_length(total);
        bar.set_position(u64::try_from(done).unwrap_or(u64::MAX));
    }

    /// Embedding-phase callback: flip to the embedding bar on first poll and
    /// track how many uploaded files remain to embed.
    fn on_poll(&self, status: StoreStatus) {
        let Some(bar) = &self.bar else {
            return;
        };
        let len = self.embed_total.load(Ordering::Relaxed);
        // store_status is store-wide, so the pending count can exceed our batch;
        // clamp to len so the bar never reads past full. Set the position before
        // any style flip so the first embedding draw shows the real position,
        // not the carried-over full upload position (a one-frame "len/len").
        let remaining = (status.pending + status.in_progress).min(len);
        bar.set_position(len - remaining);
        if !self.embedding.swap(true, Ordering::Relaxed) {
            bar.set_style(progress_style::bar("magenta"));
            bar.set_prefix("embedding files");
            bar.set_length(len);
        }
    }

    /// Clear the bar once the flow finishes.
    fn finish(&self) {
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
        }
    }
}

async fn run_grep(cli: GrepArgs) -> anyhow::Result<()> {
    let root = resolve_root(cli.path.as_deref())?;
    anyhow::ensure!(
        !at_or_above_home(&root),
        "refusing to index {} (it is at or above your home directory); run from a project directory",
        root.display(),
    );

    let palette = Palette::for_stdout();
    let theme = if cli.content {
        detect_theme(palette.color)
    } else {
        code_highlight::Theme::default()
    };

    let config = Config::default();
    let store_name = cli.store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base_url = cli
        .base_url
        .unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let store = MixedbreadStore::from_login(base_url.clone()).await?;

    // Grep reuses the shared `Query` shape; its semantic-only knobs (rerank,
    // agentic, web) are inert here, and the grep pattern travels in `text`.
    let (filter, code_scope) = resolve_scope(&cli.scope, &root)?;
    let query = Query {
        root: &root,
        store_name: &store_name,
        base_url: &base_url,
        text: &cli.pattern,
        top_k: cli.max_count.max(1),
        options: SearchOptions {
            rerank: false,
            agentic: false,
        },
        sync: !cli.no_sync,
        include_web: false,
        filters: filter.as_ref(),
        code_scope,
        index_timeout: INDEX_TIMEOUT,
    };
    let grep_options = GrepOptions {
        case_sensitive: cli.case_sensitive,
        targets: GrepTargets::Text,
    };

    // Progress UI, terminal only: identical to the semantic path so a grep on a
    // fresh tree shows the same indexing feedback. Piped output stays clean.
    let progress = IndexProgress::new();

    let hits = search_core::index_and_grep(
        &store,
        &query,
        grep_options,
        &config,
        |done, total| progress.on_upload(done, total),
        |status| progress.on_poll(status),
    )
    .await?;
    progress.finish();

    print_hits(&hits, cli.json, cli.content, &palette, &root, theme)?;

    Ok(())
}

fn resolve_root(path: Option<&str>) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let root = match path {
        Some(p) if Path::new(p).is_absolute() => PathBuf::from(p),
        Some(p) => cwd.join(p),
        None => cwd,
    };
    Ok(root.canonicalize().unwrap_or(root))
}

fn at_or_above_home(path: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let home = home.canonicalize().unwrap_or(home);
    path == home || home.starts_with(path)
}

/// Styles for one result, resolved once against stdout's color support.
///
/// A plain palette holds default `Style`s, whose `render()`/`render_reset()`
/// emit nothing, so the same `render` path produces clean text when output is
/// piped or `NO_COLOR` is set.
struct Palette {
    /// File path or web URL.
    path: Style,
    /// The `:start-end` line range after a path.
    range: Style,
    /// The `(NN.NN% match)` score suffix.
    score: Style,
    /// The left line-number gutter under `-c` content.
    gutter: Style,
    /// Whether ANSI color is enabled; forwarded to the syntax highlighter.
    color: bool,
}

impl Palette {
    /// Resolve color support from stdout, the stream results print to.
    fn for_stdout() -> Self {
        // `AutoStream::choice` honors `NO_COLOR`, `CLICOLOR_FORCE`, and whether
        // stdout is a TTY, so a single check covers every disable path.
        let choice = anstream::AutoStream::choice(&std::io::stdout());
        if choice == anstream::ColorChoice::Never {
            Self::plain()
        } else {
            Self::colored()
        }
    }

    /// No-op styles: every `render()` is empty, so output carries no escapes.
    const fn plain() -> Self {
        let none = Style::new();
        Self {
            path: none,
            range: none,
            score: none,
            gutter: none,
            color: false,
        }
    }

    /// The interactive palette: bold cyan path, dim range, green score.
    fn colored() -> Self {
        Self {
            path: Style::new().bold().fg_color(Some(AnsiColor::Cyan.into())),
            range: Style::new().dimmed(),
            score: Style::new().fg_color(Some(AnsiColor::Green.into())),
            gutter: Style::new().dimmed(),
            color: true,
        }
    }

    /// Higher relevance gets a brighter, bold green so strong hits stand out;
    /// a plain palette stays plain.
    fn score_for(&self, fraction: f32) -> Style {
        if self.score == Style::new() || fraction < 0.8 {
            self.score
        } else {
            Style::new()
                .bold()
                .fg_color(Some(AnsiColor::BrightGreen.into()))
        }
    }
}

/// Wrap `text` in `style`'s ANSI codes; a plain style yields `text` unchanged.
fn paint(style: Style, text: &str) -> String {
    format!("{}{text}{}", style.render(), style.render_reset())
}

/// Resolve the islands theme variant from the terminal background.
///
/// Probes the terminal background once via [`terminal_theme`] and maps it to the
/// highlighter's palette. Returns the dark default when color is off, leaving the
/// TTY-gating and luma probe to the shared crate so a piped or unsupported
/// terminal never blocks on a reply that will not come.
fn detect_theme(color: bool) -> code_highlight::Theme {
    if !color {
        return code_highlight::Theme::Dark;
    }
    match terminal_theme::detect() {
        terminal_theme::Theme::Light => code_highlight::Theme::Light,
        terminal_theme::Theme::Dark => code_highlight::Theme::Dark,
    }
}

/// Print hits as a JSON array (`--json`) or the human listing.
///
/// Shared by the semantic and grep paths so the machine-readable contract is
/// emitted in exactly one place.
///
/// # Errors
/// Returns an error if JSON serialization of the hits fails.
fn print_hits(
    hits: &[DisplayHit],
    json: bool,
    show_content: bool,
    palette: &Palette,
    root: &Path,
    theme: code_highlight::Theme,
) -> anyhow::Result<()> {
    if json {
        // Machine-readable mode: one JSON array on stdout for the eval harness
        // and any other consumer, instead of the human listing.
        println!("{}", search_core::hits_to_json(hits)?);
    } else {
        for hit in hits {
            println!("{}", render(hit, show_content, palette, root, theme));
        }
    }
    Ok(())
}

fn render(
    hit: &DisplayHit,
    show_content: bool,
    palette: &Palette,
    root: &Path,
    theme: code_highlight::Theme,
) -> String {
    // Only local code gets the `./path` prefix; web URLs and record titles
    // (Slack threads, Linear issues) print as-is.
    let prefix = if hit.source == Source::Code { "./" } else { "" };
    let path = paint(palette.path, &format!("{prefix}{}", hit.label));

    // `start_line` is 0-based and `num_lines` is a line count, so the displayed
    // range is the 1-based inclusive span `[start + 1, start + num]`. A
    // single-line chunk collapses to one number rather than `:n-n`.
    let location = match (hit.start_line, hit.num_lines) {
        (Some(start), Some(num)) => {
            let first = start + 1;
            let last = start + num.max(1);
            let range = if last <= first {
                format!(":{first}")
            } else {
                format!(":{first}-{last}")
            };
            paint(palette.range, &range)
        }
        (Some(start), None) => paint(palette.range, &format!(":{}", start + 1)),
        _ => String::new(),
    };

    let percent = hit.score * 100.0;
    let score = paint(
        palette.score_for(hit.score),
        &format!("({percent:.2}% match)"),
    );
    let head = format!("{path}{location} {score}");

    match show_content
        .then(|| render_snippet(hit, palette, root, theme))
        .flatten()
    {
        Some(body) => format!("{head}\n{body}"),
        None => head,
    }
}

/// Render the matched content as a readable block.
///
/// With color (a terminal, or `CLICOLOR_FORCE`), each line carries its 1-based
/// number in a right-aligned, dim gutter over syntax-highlighted source
/// (ripgrep/mgrep feel). Without color (piped to an agent, script, or file, or
/// `NO_COLOR`), it switches to `cat -n` style: `number<tab>line`, the same shape
/// a coding agent's file reader feeds an LLM, which tokenizes cleaner than a
/// box-drawing gutter and drops highlighting that is noise without color. Web
/// hits with no start line print as-is. Trailing whitespace is trimmed and an
/// empty snippet returns `None`, so nothing is printed.
fn render_snippet(
    hit: &DisplayHit,
    palette: &Palette,
    root: &Path,
    theme: code_highlight::Theme,
) -> Option<String> {
    let body = hit.text.trim_end();
    if body.is_empty() {
        return None;
    }

    let Some(start) = hit.start_line else {
        return Some(body.to_owned());
    };

    // Machine-readable mode: output is not a colored terminal, so a consumer is
    // a script or an LLM. Emit `cat -n` style and skip the rich path entirely
    // (no syntax highlighting, no aligned `│` gutter).
    if !palette.color {
        return Some(numbered_plain(body, start));
    }

    // Prefer syntax-highlighting the real file lines: tree-sitter gets full
    // parse context and code-highlight renders its own line-number gutter. Only
    // local code has a readable file; web/Slack/Linear hits fall through to a
    // plain gutter over the chunk text.
    if hit.source == Source::Code
        && let Some(num) = hit.num_lines
        && let Ok(source) = std::fs::read_to_string(root.join(&hit.label))
    {
        let snippet = code_highlight::highlight_lines(
            &hit.label,
            &source,
            usize::try_from(start).unwrap_or(0) + 1,
            usize::try_from(num).unwrap_or(0),
            theme,
            palette.color,
        );
        let snippet = snippet.trim_end();
        if !snippet.is_empty() {
            return Some(snippet.to_owned());
        }
    }

    let first = u64::from(start) + 1;
    let lines: Vec<&str> = body.lines().collect();
    let last = first + lines.len().saturating_sub(1) as u64;
    let width = last.to_string().len();

    let rendered = lines
        .iter()
        .enumerate()
        .map(|(offset, line)| {
            let number = first + offset as u64;
            let gutter = paint(palette.gutter, &format!("{number:>width$} │"));
            format!("{gutter} {}", line.trim_end())
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(rendered)
}

/// `cat -n` style numbering: `number<tab>line`, 1-based from `start`.
///
/// No alignment padding and no separator glyph: a tab is one token and lets a
/// downstream reader split the number from the line trivially. `start` is the
/// 0-based line index of the first body line, matching `DisplayHit::start_line`.
fn numbered_plain(body: &str, start: u32) -> String {
    let first = u64::from(start) + 1;
    body.lines()
        .enumerate()
        .map(|(offset, line)| format!("{}\t{}", first + offset as u64, line.trim_end()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use search_core::{DisplayHit, Source};

    use super::{Palette, render_snippet};

    /// A web hit so the snippet path renders the chunk text without reading a
    /// real file, isolating the gutter-vs-`cat -n` decision from the filesystem.
    fn hit(text: &str) -> DisplayHit {
        DisplayHit {
            label: "web://example".to_owned(),
            source: Source::Web,
            start_line: Some(4),
            num_lines: Some(2),
            score: 0.5,
            text: text.to_owned(),
        }
    }

    #[test]
    fn piped_content_is_cat_n_style() {
        let out = render_snippet(
            &hit("alpha\nbeta"),
            &Palette::plain(),
            Path::new("."),
            code_highlight::Theme::Dark,
        )
        .expect("snippet");
        assert_eq!(out, "5\talpha\n6\tbeta");
        assert!(!out.contains('│'), "machine output must drop the gutter glyph");
    }

    #[test]
    fn terminal_content_keeps_the_gutter() {
        let out = render_snippet(
            &hit("alpha\nbeta"),
            &Palette::colored(),
            Path::new("."),
            code_highlight::Theme::Dark,
        )
        .expect("snippet");
        assert!(out.contains('│'), "interactive output keeps the aligned gutter");
        assert!(!out.contains('\t'), "interactive output is not tab-numbered");
    }
}
