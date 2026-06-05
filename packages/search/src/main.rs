//! `search`: read-only semantic + regex search over the shared corpus store.
//!
//! `search` never indexes. It queries the store the `indexer` populates (code
//! plus agent/shell history) and projects the hits. Scope a query with
//! `--source`, `--repo`, `--user`, `--host`, or `--project`; with no selector it
//! searches the whole corpus. All ingestion lives in the separate `indexer`.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anstyle::{AnsiColor, Style};
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand};
use indicatif::ProgressBar;
use search_core::{
    CodeScope, DEFAULT_RERANK_MODEL, DEFAULT_STORE, DisplayHit, Filter, FilterSpec, GrepOptions,
    GrepTargets, Manifest, MixedbreadStore, Rerank, SearchOptions, Source, build_filter,
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
}

/// Scope selectors shared by the semantic and grep paths. With no selector the
/// query searches the whole corpus; each selector narrows it server-side.
#[derive(Debug, Args)]
struct ScopeArgs {
    /// Restrict to these sources (repeatable): code, `claude_history`, codex,
    /// shell, slack, linear, github, web.
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

    let spec = FilterSpec {
        sources,
        exclude_sources,
        repo: scope.repo.clone(),
        users,
        hosts: split_csv(&scope.hosts),
        projects: split_csv(&scope.projects),
    };
    Ok(build_filter(&spec))
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

    /// Synthesize an answer from the results instead of listing them.
    #[arg(short = 'a', long)]
    answer: bool,

    /// Disable result reranking (on by default).
    #[arg(long = "no-rerank")]
    no_rerank: bool,

    /// Reranking model to apply. Defaults to Mixedbread's listwise reranker.
    /// Ignored when `--no-rerank` is set.
    #[arg(long = "reranker", default_value_t = DEFAULT_RERANK_MODEL.to_owned())]
    reranker: String,

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
        None => run(cli.semantic).await,
    }
}

async fn run(cli: SemanticArgs) -> anyhow::Result<()> {
    // `pattern` is optional at the clap layer so the `grep` subcommand can be
    // invoked without it. A bare search still requires one: reject a missing
    // query with a clap usage error (stderr, exit 2, no backtrace) rather than
    // an `anyhow` error, whose Debug print carries a stack trace under
    // `RUST_BACKTRACE`.
    let Some(pattern) = cli.pattern else {
        Cli::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "a query is required: `search <pattern> [path]`",
            )
            .exit();
    };
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

    let store_name = cli.store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base_url = cli
        .base_url
        .unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let store = MixedbreadStore::from_login(base_url).await?;

    let filter = resolve_scope(&cli.scope)?;
    // Pure query: no local checkout is read, so code is scoped server-side and
    // the manifest is empty (it only ever held this checkout's hashes).
    let manifest = Manifest::default();
    let options = SearchOptions {
        rerank: if cli.no_rerank {
            Rerank::off()
        } else {
            Rerank::model(cli.reranker)
        },
        agentic: cli.agentic,
    };
    let top_k = cli.max_count.max(1);

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
            options,
            cli.web,
            filter.as_ref(),
            CodeScope::ServerFiltered,
        )
        .await;
        finish(bar);
        let view = view?;
        println!("{}", view.answer);
        if !view.sources.is_empty() {
            println!();
            for (index, hit) in view.sources.iter().enumerate() {
                println!("{index}: {}", render(hit, cli.content, &palette, &root, theme));
            }
        }
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
        )
        .await;
        finish(bar);
        print_hits(&hits?, cli.json, cli.content, &palette, &root, theme)
    }
}

/// A terminal-only "searching" spinner for the query round-trip; piped output
/// gets none. There is no upload or embedding phase to report any more.
fn spinner() -> Option<ProgressBar> {
    let bar = std::io::stderr().is_terminal().then(ProgressBar::new_spinner)?;
    bar.set_style(progress_style::spinner());
    bar.set_prefix("searching");
    bar.enable_steady_tick(Duration::from_millis(120));
    Some(bar)
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

    let store_name = cli.store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base_url = cli
        .base_url
        .unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let store = MixedbreadStore::from_login(base_url).await?;

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
    )
    .await;
    finish(bar);

    print_hits(&hits?, cli.json, cli.content, &palette, &root, theme)?;

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

    use search_core::{DisplayHit, Source};

    use super::{Palette, render_snippet};

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
