//! `search`: read-only semantic + regex search over the shared corpus store.
//!
//! `search` never indexes. It queries the store the `indexer` populates (code
//! plus agent/shell history) and projects the hits. Scope a query with
//! `--source`, `--repo`, `--user`, `--host`, `--project`, or a time window
//! (`--since`/`--until`); with no selector it searches the whole corpus. The
//! `recent` subcommand lists the newest records (descending timestamp) with no
//! semantic scoring. All ingestion lives in the separate `indexer`.
//!
//! Piped stdin switches to pipe-in mode: `ls | search "query"` ranks the piped
//! lines against the query semantically (via the reranking model) instead of
//! searching the corpus, so any line-oriented command's output can be searched
//! by meaning.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anstyle::{AnsiColor, Style};
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand};
use indicatif::ProgressBar;
use search_core::{
    Agentic, AgenticConfig, AskOptions, CodeScope, ContextView, DEFAULT_RERANK_MODEL,
    DEFAULT_STORE, DisplayHit, EnhancedQuery, Filter, FilterSpec, GrepOptions, GrepTargets,
    KNOWN_SOURCE_TAGS, Manifest, MixedbreadStore, RenderMode, Rerank, SearchOptions, SortBy,
    Source, build_filter, parse_time_spec,
};

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
    /// List the newest corpus records (descending timestamp), no semantic
    /// scoring: a deterministic "what happened lately" feed. Scope it with the
    /// usual selectors, e.g. `search recent --source shell --since 6h`.
    Recent(RecentArgs),
    /// Expand a hit into its surrounding conversation: the turns of the same
    /// session around the record, ordered by timestamp. Takes the hit's
    /// `external_id` (printed in the provenance line and `--json` output) or a
    /// bare session id; sources without a session (git, github) show the
    /// record's own chunks in order.
    Context(ContextArgs),
    /// Census the corpus: per-source document counts and freshness (the
    /// newest record timestamp per source). Scope it with the usual
    /// selectors, e.g. `search stats --host hil-compute-1 --since 7d`.
    Stats(StatsArgs),
}

/// Scope selectors shared by the semantic and grep paths. With no selector the
/// query searches the whole corpus; each selector narrows it server-side.
#[derive(Debug, Args)]
struct ScopeArgs {
    /// Restrict to these sources (repeatable): `claude_history`, codex, shell,
    /// `claude_debug`, git, github, slack, linear, code, web. An unknown value
    /// is an error (the store would silently return zero hits for a typo).
    #[arg(long = "source", value_name = "SOURCE")]
    sources: Vec<String>,

    /// Exclude these sources (repeatable).
    #[arg(long = "not-source", value_name = "SOURCE")]
    not_sources: Vec<String>,

    /// Restrict code to a repository slug (e.g. indexable-inc/index). With no
    /// `--repo`, code from every indexed repository is searched.
    #[arg(long)]
    repo: Option<String>,

    /// Restrict to records authored by these users (repeatable, comma-joined).
    /// Default: every user.
    #[arg(long = "user", value_name = "USER")]
    users: Vec<String>,

    /// Restrict to your own records (the current `$USER`); shorthand for
    /// `--user "$USER"`.
    #[arg(long)]
    mine: bool,

    /// Restrict to records recorded on these hosts (repeatable, comma-joined).
    #[arg(long = "host", value_name = "HOST")]
    hosts: Vec<String>,

    /// Restrict non-code sources to these project slugs (repeatable,
    /// comma-joined), e.g. a Claude transcript's project directory.
    #[arg(long = "project", value_name = "PROJECT")]
    projects: Vec<String>,

    /// Keep only records at or after this time: epoch seconds
    /// (e.g. 1781200000) or a relative span like 30m, 24h, 7d, 2w.
    #[arg(long, value_name = "TIME")]
    since: Option<String>,

    /// Keep only records at or before this time (same formats as --since).
    #[arg(long, value_name = "TIME")]
    until: Option<String>,
}

/// Resolve scope selectors into a server-side metadata filter. Code is scoped
/// entirely server-side (search never reads the local checkout), so there is no
/// manifest and no worktree mode.
fn resolve_scope(scope: &ScopeArgs) -> anyhow::Result<Option<Filter>> {
    let sources = parse_sources(&scope.sources)?;
    let exclude_sources = parse_sources(&scope.not_sources)?;

    let mut users = split_csv(&scope.users);
    if scope.mine {
        let me = std::env::var("USER")
            .map_err(|_| anyhow::anyhow!("--mine needs the USER environment variable set"))?;
        if !users.contains(&me) {
            users.push(me);
        }
    }

    let now = epoch_now();
    let spec = FilterSpec {
        sources,
        exclude_sources,
        repo: scope.repo.clone(),
        users,
        hosts: split_csv(&scope.hosts),
        projects: split_csv(&scope.projects),
        since: scope
            .since
            .as_deref()
            .map(|value| parse_time_spec(value, now))
            .transpose()?,
        until: scope
            .until
            .as_deref()
            .map(|value| parse_time_spec(value, now))
            .transpose()?,
    };
    Ok(build_filter(&spec))
}

/// The current wall clock as epoch seconds, the reference point for relative
/// `--since`/`--until` spans.
fn epoch_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Clamp explicitly: a wall clock past i64::MAX epoch seconds is not a real
    // input, and the clamp makes the conversion below infallible.
    let capped = secs.min(u64::try_from(i64::MAX).expect("i64::MAX is positive"));
    i64::try_from(capped).expect("capped at i64::MAX")
}

fn parse_sources(values: &[String]) -> anyhow::Result<Vec<Source>> {
    values
        .iter()
        // A source may arrive comma-joined (`--source code,slack`) or repeated.
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            // The store silently accepts any tag and returns zero hits, which is
            // indistinguishable from an empty corpus, so a typo must fail here.
            anyhow::ensure!(
                KNOWN_SOURCE_TAGS.contains(&value),
                "unknown source {value:?}; valid sources: {}",
                KNOWN_SOURCE_TAGS.join(", "),
            );
            Ok(Source::new(value))
        })
        .collect()
}

/// Flatten repeatable, comma-joined string selectors (`--user a,b --user c`)
/// into one list, trimming surrounding whitespace and dropping blanks.
fn split_csv(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Arguments for the default search path. Flags mirror `mgrep search`
/// where they overlap.
// A CLI naturally has many independent boolean flags; a state machine would
// obscure, not clarify, the argument surface.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Args)]
struct SemanticArgs {
    /// The query to search for. Optional at the clap layer so the `grep`
    /// subcommand can be given without it; a bare search still requires one and
    /// `run` rejects a missing query with a clap usage error.
    pattern: Option<String>,

    /// Directory to search in (defaults to the current directory).
    path: Option<String>,

    /// Maximum number of results to return.
    #[arg(short = 'm', long = "max-count", default_value_t = 10)]
    max_count: usize,

    /// Show the matched content under each result.
    #[arg(short = 'c', long)]
    content: bool,

    /// Synthesize an answer from the results instead of listing them. `[n]`
    /// citations in the answer reference the numbered source list under it.
    #[arg(short = 'a', long)]
    answer: bool,

    /// Extra instructions for the answering model (e.g. "answer in one
    /// sentence"), followed only when not in conflict with existing rules.
    /// Requires --answer.
    #[arg(long, value_name = "TEXT", requires = "answer")]
    instructions: Option<String>,

    /// Answer without `[n]` citation markers (the numbered source list still
    /// prints). Requires --answer.
    #[arg(long = "no-cite", requires = "answer")]
    no_cite: bool,

    /// Answer from text context only, skipping the backend's multimodal
    /// (image/audio/video) context handling, which is on by default. The
    /// shared corpus is text-only, so this only trims answering work.
    /// Requires --answer.
    #[arg(long = "no-multimodal", requires = "answer")]
    no_multimodal: bool,

    /// Disable result reranking (on by default).
    #[arg(long = "no-rerank")]
    no_rerank: bool,

    /// Reranking model to apply. Defaults to Mixedbread's listwise reranker.
    /// Ignored when `--no-rerank` is set.
    #[arg(long = "reranker", default_value_t = DEFAULT_RERANK_MODEL.to_owned())]
    reranker: String,

    /// Cap the result list after reranking (the reranker reads the full
    /// candidate set either way). Conflicts with `--no-rerank`.
    #[arg(long = "rerank-top-k", value_name = "N", conflicts_with = "no_rerank")]
    rerank_top_k: Option<usize>,

    /// Include web results from the hosted web store.
    #[arg(short = 'w', long)]
    web: bool,

    /// Let the backend plan and run multiple searches. Deliberately off by
    /// default on every surface (CLI, Python binding, MCP): it costs 10-23s
    /// per query (vs 3-6s reranked) and ~5x the per-query price, and may
    /// return fewer than --max-count hits (it gates results on its own judged
    /// relevance, on a different score scale than the reranker).
    #[arg(long)]
    agentic: bool,

    /// Cap the agentic search rounds (server default 3, max 10). Implies
    /// --agentic.
    #[arg(long = "agentic-max-rounds", value_name = "N")]
    agentic_max_rounds: Option<u32>,

    /// Extra instructions for the agentic search agent. Implies --agentic.
    #[arg(long = "agentic-instructions", value_name = "TEXT")]
    agentic_instructions: Option<String>,

    /// Rewrite the query server-side before embedding it (off by default;
    /// ignored under --agentic, where the agent owns query decomposition).
    #[arg(long = "rewrite-query")]
    rewrite_query: bool,

    /// Skip the store's server-side search rules for this query (applied by
    /// default).
    #[arg(long = "no-search-rules")]
    no_search_rules: bool,

    /// Enhance the query first: extract metadata filters (and, for
    /// ranking-shaped queries like "newest shell commands", a metadata sort)
    /// from the natural-language query server-side, print what was derived on
    /// stderr, then run the enhanced query. Derived filters are `AND`ed with
    /// any explicit scope selectors.
    #[arg(long)]
    enhance: bool,

    /// Emit results as a JSON array on stdout instead of the human listing.
    /// Each element is `{path, source, start_line, num_lines, score, text}`
    /// plus the provenance keys (`timestamp`, `user`, `host`, `session_id`,
    /// `external_id`, `url`, `repo`, `project`) when the record carries them.
    #[arg(long)]
    json: bool,

    /// Compact, token-frugal results: collapse repeated chunks of one document
    /// (keeping the best-scoring) and cap each snippet at 400 characters. Pair
    /// with --json for agent consumption; full chunks stay one flag away.
    #[arg(long)]
    compact: bool,

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

/// Arguments for the `recent` subcommand: a newest-first (descending
/// timestamp) listing of corpus records by metadata only. No semantic scoring
/// or reranking happens, so it is fast and deterministic; scores in the output
/// are the API's placeholder, not relevance.
#[derive(Debug, Args)]
struct RecentArgs {
    /// Maximum number of records to return.
    #[arg(short = 'm', long = "max-count", default_value_t = 20)]
    max_count: usize,

    /// Show each record's content under its heading.
    #[arg(short = 'c', long)]
    content: bool,

    /// Emit results as a JSON array on stdout (same shape as `search --json`).
    #[arg(long)]
    json: bool,

    /// Collapse repeated records and cap each snippet at 400 characters.
    #[arg(long)]
    compact: bool,

    /// Scope selectors (source/user/host/repo/project/since/until).
    #[command(flatten)]
    scope: ScopeArgs,

    /// Store name (one store holds every worktree's content).
    #[arg(long, env = "MXBAI_STORE")]
    store: Option<String>,

    /// Mixedbread API base URL.
    #[arg(long = "base-url", env = "MXBAI_BASE_URL")]
    base_url: Option<String>,
}

/// Arguments for the `context` subcommand: expand one record into the
/// conversation around it.
#[derive(Debug, Args)]
struct ContextArgs {
    /// The record to expand: a hit's `external_id`
    /// (e.g. `claude:{session}:{uuid}`), or a bare session id to list that
    /// session from its start.
    id: String,

    /// Turns of the same session to show before the record.
    #[arg(long, default_value_t = 5, value_name = "N")]
    before: usize,

    /// Turns of the same session to show after the record.
    #[arg(long, default_value_t = 5, value_name = "N")]
    after: usize,

    /// Emit the conversation as one JSON object on stdout: `{"turns": [...],
    /// "anchor": <index|null>}`, each turn shaped like a `search --json` hit.
    #[arg(long)]
    json: bool,

    /// Store name (one store holds every worktree's content).
    #[arg(long, env = "MXBAI_STORE")]
    store: Option<String>,

    /// Mixedbread API base URL.
    #[arg(long = "base-url", env = "MXBAI_BASE_URL")]
    base_url: Option<String>,
}

/// Arguments for the `stats` subcommand: a per-source census of the corpus
/// (document counts from the store's metadata facets, freshness from a
/// newest-first listing per source). Deterministic and metadata-only — no
/// semantic scoring happens.
#[derive(Debug, Args)]
struct StatsArgs {
    /// Emit the census as a JSON array on stdout: one
    /// `{source, documents, newest_timestamp}` object per source
    /// (`newest_timestamp` is epoch seconds, omitted when no record carries
    /// one).
    #[arg(long)]
    json: bool,

    /// Scope selectors (source/user/host/repo/project/since/until); the
    /// counts and freshness then describe the scoped slice.
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

    /// Emit results as a JSON array on stdout instead of the human listing.
    /// Each element is `{path, source, start_line, num_lines, score, text}`
    /// plus the provenance keys (`timestamp`, `user`, `host`, `session_id`,
    /// `external_id`, `url`, `repo`, `project`) when the record carries them.
    #[arg(long)]
    json: bool,

    /// Compact, token-frugal results: collapse repeated chunks of one document
    /// (keeping the best-scoring) and cap each snippet at 400 characters.
    #[arg(long)]
    compact: bool,

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
        Some(Command::Recent(args)) => run_recent(args).await,
        Some(Command::Context(args)) => run_context(args).await,
        Some(Command::Stats(args)) => run_stats(args).await,
        None => run(cli.semantic).await,
    }
}

/// The bare-search query. A missing one exits with a clap usage error
/// (stderr, exit 2, no backtrace) rather than an `anyhow` error, whose Debug
/// print carries a stack trace under `RUST_BACKTRACE`; `pattern` is optional
/// at the clap layer only so the subcommands can be invoked without it.
fn require_pattern(cli: &SemanticArgs) -> String {
    cli.pattern.clone().unwrap_or_else(|| {
        Cli::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "a query is required: `search <pattern> [path]`",
            )
            .exit()
    })
}

/// An authenticated store handle and the resolved store name.
struct Connection {
    store: MixedbreadStore,
    name: String,
}

/// Resolve the shared connection flags (defaulting the store name and base
/// URL) and authenticate.
async fn connect(store: Option<String>, base_url: Option<String>) -> anyhow::Result<Connection> {
    let name = store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base_url = base_url.unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let store = MixedbreadStore::from_login(base_url).await?;
    Ok(Connection { store, name })
}

/// Print a question-answering view: the synthesized answer, then its sources
/// in the same rendering the result listing uses.
fn print_answer(
    view: &search_core::AnswerView,
    content: bool,
    palette: &Palette,
    root: &Path,
    theme: code_highlight::Theme,
) {
    println!("{}", view.answer);
    if !view.sources.is_empty() {
        println!();
        for (index, hit) in view.sources.iter().enumerate() {
            println!("{index}: {}", render(hit, content, palette, root, theme));
        }
    }
}

/// The projection mode for a `--compact` flag.
const fn render_mode(compact: bool) -> RenderMode {
    if compact {
        RenderMode::Compact
    } else {
        RenderMode::Full
    }
}

async fn run(cli: SemanticArgs) -> anyhow::Result<()> {
    let pattern = require_pattern(&cli);

    // Pipe-in mode: `ls | search "query"` (or `gh issue list | search "..."`)
    // ranks the piped lines against the query semantically instead of searching
    // the indexed corpus. An empty or absent pipe (a TTY, `</dev/null`, a
    // script with stdin closed) falls through to the normal corpus search, so
    // only real piped content changes behavior.
    if let Some(docs) = piped_stdin_lines()? {
        return run_piped(&cli, &pattern, docs).await;
    }
    let root = resolve_root(cli.path.as_deref())?;

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

    let Connection {
        store,
        name: store_name,
    } = connect(cli.store.clone(), cli.base_url.clone()).await?;

    // Pure query: no local checkout is read, so code is scoped server-side and
    // the manifest is empty (it only ever held this checkout's hashes).
    let manifest = Manifest::default();
    let options = search_options(&cli);
    let top_k = cli.max_count.max(1);

    let scope_filter = resolve_scope(&cli.scope)?;
    let EnhancedScope {
        pattern,
        filter,
        sort,
    } = if cli.enhance {
        anyhow::ensure!(
            !cli.web,
            "--enhance is not supported with --web; filters are derived from the corpus store's metadata",
        );
        enhance_scope(&store, &store_name, pattern, scope_filter).await?
    } else {
        EnhancedScope {
            pattern,
            filter: scope_filter,
            sort: None,
        }
    };

    // A sort item means the query asked for a metadata ranking, not a semantic
    // match ("newest shell commands"): run the deterministic ranked listing
    // under the merged filter instead of embedding the query.
    if let Some(sort) = sort {
        anyhow::ensure!(
            !cli.answer,
            "the enhanced query asks for a metadata ranking, which --answer cannot synthesize from; drop --answer or rephrase the query",
        );
        let bar = spinner();
        let hits = search_core::ranked(
            &store,
            &store_name,
            top_k,
            filter.as_ref(),
            &sort,
            render_mode(cli.compact),
        )
        .await;
        finish(bar);
        return print_hits(&hits?, cli.json, cli.content, &palette, &root, theme);
    }

    let bar = spinner();
    if cli.answer {
        anyhow::ensure!(
            !cli.json,
            "--json is not supported with --answer; pass one or the other",
        );
        let view = search_core::ask(
            &store,
            &store_name,
            &manifest,
            &pattern,
            top_k,
            ask_options(&cli, options),
            cli.web,
            filter.as_ref(),
            CodeScope::ServerFiltered,
        )
        .await;
        finish(bar);
        print_answer(&view?, cli.content, &palette, &root, theme);
        Ok(())
    } else {
        let hits = search_core::semantic(
            &store,
            &store_name,
            &manifest,
            &pattern,
            top_k,
            options,
            cli.web,
            filter.as_ref(),
            CodeScope::ServerFiltered,
            render_mode(cli.compact),
        )
        .await;
        finish(bar);
        print_hits(&hits?, cli.json, cli.content, &palette, &root, theme)
    }
}

/// Run the `recent` subcommand: list the newest records matching the scope,
/// descending by timestamp, via the store's metadata-only chunk listing.
async fn run_recent(cli: RecentArgs) -> anyhow::Result<()> {
    let root = resolve_root(None)?;
    let palette = Palette::for_stdout();
    let theme = if cli.content {
        detect_theme(palette.color)
    } else {
        code_highlight::Theme::default()
    };

    let Connection {
        store,
        name: store_name,
    } = connect(cli.store, cli.base_url).await?;
    let filter = resolve_scope(&cli.scope)?;

    let bar = spinner();
    let hits = search_core::recent(
        &store,
        &store_name,
        cli.max_count.max(1),
        filter.as_ref(),
        render_mode(cli.compact),
    )
    .await;
    finish(bar);
    print_hits(&hits?, cli.json, cli.content, &palette, &root, theme)
}

/// Run the `context` subcommand: fetch the conversation around a record and
/// render it oldest-first, the requested record marked in the margin.
async fn run_context(cli: ContextArgs) -> anyhow::Result<()> {
    let palette = Palette::for_stdout();
    let store_name = cli.store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base_url = cli
        .base_url
        .unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let store = MixedbreadStore::from_login(base_url).await?;

    let bar = spinner();
    let view = search_core::context(&store, &store_name, &cli.id, cli.before, cli.after).await;
    finish(bar);
    let view = view?;

    if cli.json {
        // Machine-readable mode: the whole view as one JSON object, so the
        // anchor position travels with the turns.
        println!("{}", serde_json::to_string(&view)?);
    } else {
        println!("{}", render_conversation(&view, &palette));
    }
    Ok(())
}

/// Run the `stats` subcommand: census the corpus per source (document counts
/// and freshness) and print a table, or one JSON array under `--json`.
async fn run_stats(cli: StatsArgs) -> anyhow::Result<()> {
    let palette = Palette::for_stdout();
    let Connection {
        store,
        name: store_name,
    } = connect(cli.store, cli.base_url).await?;
    let filter = resolve_scope(&cli.scope)?;

    let bar = spinner_with("counting");
    let rows = search_core::stats(&store, &store_name, filter.as_ref()).await;
    finish(bar);
    let rows = rows?;

    // Each source is counted by its own bounded backend scan; a source at
    // the bound reports a lower bound, rendered with `≥` below. Said on
    // stderr too so `--json` consumers reading stdout still get the marked
    // rows while a human sees why.
    let any_truncated = rows.iter().any(|row| row.truncated);
    if any_truncated {
        eprintln!(
            "note: counts of {} hit the census scan cap and are lower bounds",
            search_core::FACETS_MAX_FILES,
        );
    }

    if cli.json {
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }

    let total: u64 = rows.iter().map(|row| row.documents).sum();
    let render_count =
        |documents: u64, truncated: bool| format!("{}{documents}", if truncated { "\u{2265}" } else { "" });
    let name_width = rows
        .iter()
        .map(|row| row.source.len())
        .chain(["source".len(), "total".len()])
        .max()
        .unwrap_or_default();
    let count_width = rows
        .iter()
        .map(|row| render_count(row.documents, row.truncated).chars().count())
        .chain([
            "documents".len(),
            render_count(total, any_truncated).chars().count(),
        ])
        .max()
        .unwrap_or_default();
    println!(
        "{}",
        paint(
            palette.range,
            &format!("{:<name_width$}  {:>count_width$}  newest", "source", "documents"),
        )
    );
    let now = epoch_now();
    for row in &rows {
        let newest = row.newest_timestamp.map_or_else(
            || "-".to_owned(),
            |ts| format!("{} ({} ago)", format_epoch(ts), format_age(now, ts)),
        );
        println!(
            "{:<name_width$}  {:>count_width$}  {newest}",
            row.source,
            render_count(row.documents, row.truncated),
        );
    }
    println!(
        "{}",
        paint(
            palette.range,
            &format!(
                "{:<name_width$}  {:>count_width$}",
                "total",
                render_count(total, any_truncated),
            ),
        )
    );
    Ok(())
}

/// Compact age of an epoch timestamp relative to `now`: `42s`, `10m`, `3h`,
/// `12d`. A timestamp at or past `now` (clock skew between recording hosts)
/// reads `0s`.
fn format_age(now: i64, timestamp: i64) -> String {
    let delta = now.saturating_sub(timestamp).max(0);
    match delta {
        0..60 => format!("{delta}s"),
        60..3_600 => format!("{}m", delta / 60),
        3_600..86_400 => format!("{}h", delta / 3_600),
        _ => format!("{}d", delta / 86_400),
    }
}

/// Render a context view as a conversation: an optional session header, then
/// one block per turn — a dim `timestamp · title` heading over the turn's
/// text, indented so the headings carry the eye. The requested record is
/// marked with `>` in the margin and a highlighted heading.
fn render_conversation(view: &ContextView, palette: &Palette) -> String {
    let mut blocks: Vec<String> = Vec::new();

    // One session header instead of repeating the identity on every turn:
    // every turn of a window shares the session, source, and author.
    if let Some(first) = view.turns.first() {
        let mut parts: Vec<String> = Vec::new();
        if let Some(session) = &first.session_id {
            parts.push(format!("session {session}"));
        }
        parts.push(first.source.as_str().to_owned());
        match (first.user.as_deref(), first.host.as_deref()) {
            (Some(user), Some(host)) => parts.push(format!("{user}@{host}")),
            (Some(user), None) => parts.push(user.to_owned()),
            (None, Some(host)) => parts.push(format!("@{host}")),
            (None, None) => {}
        }
        if let Some(project) = &first.project {
            parts.push(format!("project={project}"));
        }
        blocks.push(paint(palette.range, &parts.join(" \u{b7} ")));
    }

    for (index, turn) in view.turns.iter().enumerate() {
        let is_anchor = view.anchor == Some(index);
        let marker = if is_anchor { ">" } else { " " };

        let mut head: Vec<String> = Vec::new();
        if let Some(ts) = turn.timestamp {
            head.push(format_epoch(ts));
        }
        if !turn.label.is_empty() {
            head.push(turn.label.clone());
        }
        // The anchor heading uses the path style so the requested record
        // stands out; the rest stay dim, letting the bodies carry the read.
        let style = if is_anchor { palette.path } else { palette.range };
        let mut block = format!("{marker} {}", paint(style, &head.join(" \u{b7} ")));

        for line in turn.text.trim_end().lines() {
            block.push_str("\n    ");
            block.push_str(line.trim_end());
        }
        blocks.push(block);
    }

    blocks.join("\n\n")
}

/// A terminal-only "searching" spinner for the query round-trip; piped output
/// gets none. There is no upload or embedding phase to report any more.
fn spinner() -> Option<ProgressBar> {
    spinner_with("searching")
}

/// [`spinner`] with an explicit phase prefix (e.g. "enhancing").
fn spinner_with(prefix: &'static str) -> Option<ProgressBar> {
    let bar = std::io::stderr()
        .is_terminal()
        .then(ProgressBar::new_spinner)?;
    bar.set_style(progress_style::spinner());
    bar.set_prefix(prefix);
    bar.enable_steady_tick(Duration::from_millis(120));
    Some(bar)
}

/// Resolve the agentic flags into the typed selection: any tuning flag
/// implies agentic search on (with that tuning); otherwise the plain toggle.
fn agentic_selection(cli: &SemanticArgs) -> Agentic {
    if cli.agentic_max_rounds.is_some() || cli.agentic_instructions.is_some() {
        Agentic::Config(AgenticConfig {
            max_rounds: cli.agentic_max_rounds,
            instructions: cli.agentic_instructions.clone(),
            ..AgenticConfig::default()
        })
    } else {
        Agentic::Toggle(cli.agentic)
    }
}

/// Resolve the answering flags into the backend ask options. Unset toggles
/// stay `None`, so the backend defaults (citations on, multimodal on) apply
/// with no change to the wire body.
fn ask_options(cli: &SemanticArgs, search: SearchOptions) -> AskOptions {
    AskOptions {
        search,
        cite: cli.no_cite.then_some(false),
        multimodal: cli.no_multimodal.then_some(false),
        instructions: cli.instructions.clone(),
    }
}

/// Resolve the option flags into the backend search options.
fn search_options(cli: &SemanticArgs) -> SearchOptions {
    SearchOptions {
        rerank: if cli.no_rerank {
            Rerank::off()
        } else {
            Rerank::Model {
                model: cli.reranker.clone(),
                top_k: cli.rerank_top_k,
            }
        },
        agentic: agentic_selection(cli),
        rewrite_query: cli.rewrite_query,
        apply_search_rules: !cli.no_search_rules,
    }
}

/// What `--enhance` resolved the query to: the (possibly rewritten) pattern,
/// the scope filter with the derived conditions folded in, and a metadata
/// sort when the query was ranking-shaped rather than semantic.
struct EnhancedScope {
    pattern: String,
    filter: Option<Filter>,
    sort: Option<SortBy>,
}

/// Run query enhancement and fold the result into the search scope: report
/// what was derived on stderr, `AND` the derived filter with the explicit
/// selectors, and surface a sort item as a [`SortBy`].
async fn enhance_scope(
    store: &MixedbreadStore,
    store_name: &str,
    pattern: String,
    scope_filter: Option<Filter>,
) -> anyhow::Result<EnhancedScope> {
    let bar = spinner_with("enhancing");
    let enhanced = store
        .enhance_query(&[store_name.to_owned()], &pattern, None)
        .await;
    finish(bar);
    let enhanced = enhanced?;

    let derived = enhanced.filter();
    report_enhancement(&enhanced, derived.as_ref())?;
    let filter = merge_filters(scope_filter, derived);
    Ok(match enhanced {
        EnhancedQuery::Query { query, .. } => EnhancedScope {
            pattern: query,
            filter,
            sort: None,
        },
        EnhancedQuery::Sort {
            rank_by, direction, ..
        } => EnhancedScope {
            pattern,
            filter,
            sort: Some(SortBy {
                field: rank_by,
                ascending: direction.is_ascending(),
            }),
        },
    })
}

/// `AND` the explicit scope selectors with what enhancement derived; either
/// side may be absent.
fn merge_filters(scope: Option<Filter>, derived: Option<Filter>) -> Option<Filter> {
    match (scope, derived) {
        (Some(scope), Some(derived)) => Some(Filter::all(vec![scope, derived])),
        (scope, None) => scope,
        (None, derived) => derived,
    }
}

/// Print what `--enhance` derived, on stderr so `--json` keeps stdout a clean
/// array. The filter prints as the exact JSON sent to the API, copy-pastable
/// into other tools that take the recursive filter shape.
///
/// # Errors
/// Returns an error if the derived filter fails to serialize.
fn report_enhancement(enhanced: &EnhancedQuery, derived: Option<&Filter>) -> anyhow::Result<()> {
    match enhanced {
        EnhancedQuery::Query { query, .. } => eprintln!("enhanced query: {query}"),
        EnhancedQuery::Sort {
            rank_by, direction, ..
        } => {
            let direction = if direction.is_ascending() {
                "ascending"
            } else {
                "descending"
            };
            eprintln!("enhanced sort: {rank_by} ({direction})");
        }
    }
    match derived {
        Some(filter) => eprintln!("derived filter: {}", serde_json::to_string(filter)?),
        None => eprintln!("derived filter: (none)"),
    }
    Ok(())
}

/// Clear the spinner, if any, before printing results.
fn finish(bar: Option<ProgressBar>) {
    if let Some(bar) = bar {
        bar.finish_and_clear();
    }
}

async fn run_grep(cli: GrepArgs) -> anyhow::Result<()> {
    let root = resolve_root(cli.path.as_deref())?;

    let palette = Palette::for_stdout();
    let theme = if cli.content {
        detect_theme(palette.color)
    } else {
        code_highlight::Theme::default()
    };

    let Connection {
        store,
        name: store_name,
    } = connect(cli.store, cli.base_url).await?;

    let filter = resolve_scope(&cli.scope)?;
    let manifest = Manifest::default();
    let grep_options = GrepOptions {
        case_sensitive: cli.case_sensitive,
        targets: GrepTargets::Text,
    };

    let bar = spinner();
    let hits = search_core::grep(
        &store,
        &store_name,
        &manifest,
        &cli.pattern,
        cli.max_count.max(1),
        grep_options,
        filter.as_ref(),
        CodeScope::ServerFiltered,
        render_mode(cli.compact),
    )
    .await;
    finish(bar);

    print_hits(&hits?, cli.json, cli.content, &palette, &root, theme)?;

    Ok(())
}

/// The piped stdin as candidate documents, or None when there is nothing to
/// rank: stdin is a TTY (interactive use), or the pipe carried no non-blank
/// line (`</dev/null`, a script with stdin closed), in which case the normal
/// corpus search runs.
fn piped_stdin_lines() -> anyhow::Result<Option<Vec<String>>> {
    use std::io::Read as _;

    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(None);
    }
    let mut piped = String::new();
    stdin.lock().read_to_string(&mut piped)?;
    let docs = split_documents(&piped);
    Ok(if docs.is_empty() { None } else { Some(docs) })
}

/// Split piped text into candidate documents: one per line, whitespace-trimmed,
/// blanks dropped. Line-oriented input is what a pipe carries (`ls`, `gh issue
/// list`, a log), so each line ranks as its own candidate.
fn split_documents(piped: &str) -> Vec<String> {
    piped
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// True when any scope selector was given. Pipe mode reads no store, so a
/// scope selector there can only mislead; this powers its rejection.
const fn scope_is_set(scope: &ScopeArgs) -> bool {
    !scope.sources.is_empty()
        || !scope.not_sources.is_empty()
        || scope.repo.is_some()
        || !scope.users.is_empty()
        || scope.mine
        || !scope.hosts.is_empty()
        || !scope.projects.is_empty()
        || scope.since.is_some()
        || scope.until.is_some()
}

/// Rank piped lines against the query with the reranking model and print the
/// top hits. No store is read: the candidates are the caller's own lines, and
/// only the ranking comes from the API, so scope/store selectors do not apply.
async fn run_piped(cli: &SemanticArgs, pattern: &str, docs: Vec<String>) -> anyhow::Result<()> {
    anyhow::ensure!(
        !cli.answer,
        "--answer is not supported with piped input; pipe mode ranks the piped lines",
    );
    anyhow::ensure!(
        !cli.no_rerank,
        "--no-rerank is not supported with piped input; ranking the piped lines IS the rerank",
    );
    // The remaining corpus-only knobs are rejected rather than silently
    // ignored: with piped input nothing is searched but the piped lines, so a
    // path argument or scope selector would never narrow anything and a user
    // passing one is asking for a corpus search they are not getting.
    anyhow::ensure!(
        cli.path.is_none(),
        "a path argument is not supported with piped input; pipe mode ranks the piped lines, not a directory",
    );
    anyhow::ensure!(
        !cli.web,
        "--web is not supported with piped input; pipe mode ranks the piped lines, not the web store",
    );
    anyhow::ensure!(
        !cli.agentic,
        "--agentic is not supported with piped input; pipe mode runs a single rerank of the piped lines",
    );
    anyhow::ensure!(
        !cli.enhance,
        "--enhance is not supported with piped input; pipe mode ranks the piped lines, not the corpus",
    );
    anyhow::ensure!(
        !cli.rewrite_query
            && !cli.no_search_rules
            && cli.rerank_top_k.is_none()
            && cli.agentic_max_rounds.is_none()
            && cli.agentic_instructions.is_none(),
        "search-option flags (--rewrite-query/--no-search-rules/--rerank-top-k/--agentic-*) are not supported with piped input; pipe mode runs a single rerank of the piped lines",
    );
    anyhow::ensure!(
        !scope_is_set(&cli.scope),
        "scope selectors (--source/--not-source/--repo/--user/--mine/--host/--project) are not supported with piped input; pipe mode ranks the piped lines, not the corpus",
    );

    let palette = Palette::for_stdout();
    let base_url = cli
        .base_url
        .clone()
        .unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let client = mixedbread::Client::from_login(base_url).await?;

    let bar = spinner();
    let hits = client
        .rerank(&cli.reranker, pattern, &docs, cli.max_count.max(1))
        .await;
    finish(bar);
    let hits = hits?;

    if cli.json {
        // Machine-readable mode: one JSON array on stdout. `index` is the
        // 0-based position of the line in the piped input, so a consumer can
        // map a hit back to its original row.
        let items: Vec<serde_json::Value> = hits
            .iter()
            .filter_map(|hit| {
                docs.get(hit.index).map(|text| {
                    serde_json::json!({
                        "index": hit.index,
                        "score": hit.score,
                        "text": text,
                    })
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&items)?);
        return Ok(());
    }
    for hit in &hits {
        let Some(text) = docs.get(hit.index) else {
            continue;
        };
        let percent = hit.score * 100.0;
        let score = paint(
            palette.score_for(hit.score),
            &format!("({percent:.2}% match)"),
        );
        println!("{text} {score}");
    }
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
    let prefix = if hit.source.is_code() { "./" } else { "" };
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
    let mut out = format!("{path}{location} {score}");

    if let Some(line) = provenance_line(hit, palette) {
        out.push('\n');
        out.push_str(&line);
    }
    if let Some(body) = show_content
        .then(|| render_snippet(hit, palette, root, theme))
        .flatten()
    {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

/// One dim line of provenance under a hit: source tag, UTC timestamp,
/// `user@host`, and the follow-up identifiers (session, repo, project, URL)
/// when the record carries them. This is what lets a reader judge staleness
/// and pivot from a hit to its origin without `--json`.
fn provenance_line(hit: &DisplayHit, palette: &Palette) -> Option<String> {
    let mut parts: Vec<String> = vec![hit.source.as_str().to_owned()];
    if let Some(ts) = hit.timestamp {
        parts.push(format_epoch(ts));
    }
    match (hit.user.as_deref(), hit.host.as_deref()) {
        (Some(user), Some(host)) => parts.push(format!("{user}@{host}")),
        (Some(user), None) => parts.push(user.to_owned()),
        (None, Some(host)) => parts.push(format!("@{host}")),
        (None, None) => {}
    }
    if let Some(session) = &hit.session_id {
        parts.push(format!("session={session}"));
    }
    if let Some(repo) = &hit.repo {
        parts.push(format!("repo={repo}"));
    }
    if let Some(project) = &hit.project {
        parts.push(format!("project={project}"));
    }
    if let Some(url) = &hit.url {
        parts.push(url.clone());
    }
    // A bare source tag carries no information the listing lacks; only print
    // the line when the record contributed something.
    if parts.len() == 1 {
        return None;
    }
    Some(paint(palette.range, &format!("  {}", parts.join(" \u{b7} "))))
}

/// Format an epoch-second timestamp as a UTC instant (`2026-06-12 03:11 UTC`),
/// falling back to the raw integer when out of chrono's range.
fn format_epoch(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0).map_or_else(
        || timestamp.to_string(),
        |instant| instant.format("%Y-%m-%d %H:%M UTC").to_string(),
    )
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
    if hit.source.is_code()
        && let Some(num) = hit.num_lines
        && let Ok(source) = std::fs::read_to_string(root.join(&hit.label))
    {
        // `start`/`num` are `u32` line counts; `u32` always fits in `usize` on
        // the 64-bit Unix targets we support, so the widening `as` is lossless.
        let snippet = code_highlight::highlight_lines(
            &hit.label,
            &source,
            start as usize + 1,
            num as usize,
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

    use search_core::{ContextView, DisplayHit, Source};

    use super::{
        Palette, ScopeArgs, format_age, format_epoch, parse_sources, provenance_line,
        render_conversation, render_snippet, scope_is_set, split_documents,
    };

    /// A web hit so the snippet path renders the chunk text without reading a
    /// real file, isolating the gutter-vs-`cat -n` decision from the filesystem.
    fn hit(text: &str) -> DisplayHit {
        DisplayHit {
            label: "web://example".to_owned(),
            source: Source::web(),
            start_line: Some(4),
            num_lines: Some(2),
            score: 0.5,
            text: text.to_owned(),
            timestamp: None,
            user: None,
            host: None,
            session_id: None,
            external_id: None,
            url: None,
            repo: None,
            project: None,
        }
    }

    #[test]
    fn split_documents_is_per_trimmed_nonblank_line() {
        // Pipe-in mode ranks one candidate per line: trailing newlines, blank
        // separator lines, and surrounding whitespace must not produce empty or
        // padded candidates (the API would happily rank them).
        assert_eq!(
            split_documents("  Cargo.toml \n\nsrc/main.rs\n\n\n"),
            vec!["Cargo.toml".to_owned(), "src/main.rs".to_owned()],
        );
        assert!(split_documents("\n  \n").is_empty());
        assert!(split_documents("").is_empty());
    }

    #[test]
    fn scope_is_set_detects_every_selector() {
        // Pipe mode rejects scope selectors instead of silently ignoring them;
        // each selector field must trip the check, and the empty scope must not
        // (or pipe mode would always error).
        fn empty() -> ScopeArgs {
            ScopeArgs {
                sources: vec![],
                not_sources: vec![],
                repo: None,
                users: vec![],
                mine: false,
                hosts: vec![],
                projects: vec![],
                since: None,
                until: None,
            }
        }
        assert!(!scope_is_set(&empty()));

        let set: [fn(&mut ScopeArgs); 9] = [
            |s| s.sources = vec!["code".to_owned()],
            |s| s.not_sources = vec!["web".to_owned()],
            |s| s.repo = Some("indexable-inc/index".to_owned()),
            |s| s.users = vec!["andrew".to_owned()],
            |s| s.mine = true,
            |s| s.hosts = vec!["devbox".to_owned()],
            |s| s.projects = vec!["index".to_owned()],
            |s| s.since = Some("24h".to_owned()),
            |s| s.until = Some("1781200000".to_owned()),
        ];
        for (which, apply) in set.iter().enumerate() {
            let mut scope = empty();
            apply(&mut scope);
            assert!(scope_is_set(&scope), "selector {which} not detected");
        }
    }

    #[test]
    fn unknown_source_errors_and_lists_the_valid_tags() {
        // A mistyped source is silently accepted by the store and returns zero
        // hits; the CLI must reject it loudly instead.
        let err = parse_sources(&["claude-history".to_owned()]).expect_err("must reject");
        let message = err.to_string();
        assert!(message.contains("claude-history"), "{message}");
        assert!(message.contains("claude_history"), "{message}");
        assert!(message.contains("shell"), "{message}");

        // Canonical tags pass, comma-joined or repeated.
        let parsed =
            parse_sources(&["shell,claude_history".to_owned(), "code".to_owned()]).expect("valid");
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn provenance_line_carries_identity_and_skips_empty_records() {
        let mut with_meta = hit("body");
        with_meta.source = Source::new("claude_history");
        with_meta.timestamp = Some(1_781_222_222);
        with_meta.user = Some("andrew".to_owned());
        with_meta.host = Some("hydra".to_owned());
        with_meta.session_id = Some("sess-1".to_owned());
        let line = provenance_line(&with_meta, &Palette::plain()).expect("line");
        assert!(line.contains("claude_history"), "{line}");
        assert!(line.contains("2026-06-11 23:57 UTC"), "{line}");
        assert!(line.contains("andrew@hydra"), "{line}");
        assert!(line.contains("session=sess-1"), "{line}");

        // A hit with no provenance metadata prints no extra line.
        assert!(provenance_line(&hit("body"), &Palette::plain()).is_none());
    }

    #[test]
    fn format_epoch_is_utc_minutes() {
        assert_eq!(format_epoch(1_781_222_222), "2026-06-11 23:57 UTC");
    }

    #[test]
    fn format_age_picks_the_unit_and_clamps_skew() {
        let now = 1_000_000;
        assert_eq!(format_age(now, now - 59), "59s");
        assert_eq!(format_age(now, now - 60), "1m");
        assert_eq!(format_age(now, now - 7_200), "2h");
        assert_eq!(format_age(now, now - 200_000), "2d");
        // A recording host ahead of this one must not print a negative age.
        assert_eq!(format_age(now, now + 30), "0s");
    }

    #[test]
    fn conversation_marks_the_anchor_and_indents_bodies() {
        let mut earlier = hit("the question\nwith two lines");
        earlier.source = Source::new("claude_history");
        earlier.label = "user @ proj: the question".to_owned();
        earlier.timestamp = Some(1_781_222_222);
        earlier.session_id = Some("sess-1".to_owned());
        earlier.user = Some("andrew".to_owned());
        let mut anchor = hit("the answer");
        anchor.source = Source::new("claude_history");
        anchor.label = "assistant @ proj: the answer".to_owned();
        anchor.session_id = Some("sess-1".to_owned());

        let view = ContextView {
            turns: vec![earlier, anchor],
            anchor: Some(1),
        };
        let out = render_conversation(&view, &Palette::plain());

        // Session identity prints once, up top, not per turn.
        assert!(out.starts_with("session sess-1"), "{out}");
        assert!(out.contains("claude_history"), "{out}");
        // Turns render oldest-first with indented bodies; only the anchor
        // carries the margin marker.
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("  2026-06-11 23:57 UTC")),
            "{out}"
        );
        assert!(lines.contains(&"    the question"), "{out}");
        assert!(lines.contains(&"    with two lines"), "{out}");
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.starts_with("> "))
                .copied()
                .collect::<Vec<_>>(),
            vec!["> assistant @ proj: the answer"],
        );
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
        assert!(
            !out.contains('│'),
            "machine output must drop the gutter glyph"
        );
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
        assert!(
            out.contains('│'),
            "interactive output keeps the aligned gutter"
        );
        assert!(
            !out.contains('\t'),
            "interactive output is not tab-numbered"
        );
    }
}
