//! `semantic-search`: a daemon-free, content-addressed semantic code search.
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
use clap::Parser;
use indicatif::ProgressBar;
use semantic_search_core::{
    Config, DEFAULT_STORE, DisplayHit, MixedbreadStore, Query, SearchOptions, StoreStatus,
};

/// How long to wait for freshly uploaded files to finish embedding.
const INDEX_TIMEOUT: Duration = Duration::from_mins(2);

/// Command-line arguments. Flags mirror `mgrep search` where they overlap.
// A CLI naturally has many independent boolean flags; a state machine would
// obscure, not clarify, the argument surface.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Parser)]
#[command(name = "semantic-search", about, version)]
struct Cli {
    /// The query to search for.
    pattern: String,

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

    /// Store name (one store holds every worktree's content).
    #[arg(long, env = "MXBAI_STORE")]
    store: Option<String>,

    /// Mixedbread API base URL.
    #[arg(long = "base-url", env = "MXBAI_BASE_URL")]
    base_url: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run(Cli::parse()).await
}

async fn run(cli: Cli) -> anyhow::Result<()> {
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
    let store = MixedbreadStore::from_login(base_url).await?;

    let query = Query {
        root: &root,
        store_name: &store_name,
        text: &cli.pattern,
        top_k: cli.max_count.max(1),
        options: SearchOptions {
            rerank: !cli.no_rerank,
            agentic: cli.agentic,
        },
        sync: !cli.no_sync,
        include_web: cli.web,
        index_timeout: INDEX_TIMEOUT,
    };

    // Progress UI, terminal only: an upload bar that flips to an embedding
    // bar on the first poll. Piped output stays clean (no bar).
    let bar = std::io::stderr()
        .is_terminal()
        .then(ProgressBar::new_spinner);
    if let Some(bar) = &bar {
        bar.set_style(progress_style::bar("cyan"));
        bar.set_prefix("indexing files");
    }
    let embedding = AtomicBool::new(false);
    // Captured during upload so the embedding bar knows its length: the number
    // of files uploaded is exactly the number that will embed.
    let embed_total = AtomicU64::new(0);
    let on_upload = |done: usize, total: usize| {
        if let (Some(bar), true) = (&bar, total > 0) {
            let total = u64::try_from(total).unwrap_or(u64::MAX);
            embed_total.store(total, Ordering::Relaxed);
            bar.set_length(total);
            bar.set_position(u64::try_from(done).unwrap_or(u64::MAX));
        }
    };
    let on_poll = |status: StoreStatus| {
        if let Some(bar) = &bar {
            let len = embed_total.load(Ordering::Relaxed);
            // store_status is store-wide, so the pending count can exceed our
            // batch; clamp to len so the bar never reads past full. Set the
            // position before any style flip so the first embedding draw shows
            // the real position, not the carried-over full upload position
            // (which would flash as "len/len" for one frame).
            let remaining = (status.pending + status.in_progress).min(len);
            bar.set_position(len - remaining);
            if !embedding.swap(true, Ordering::Relaxed) {
                bar.set_style(progress_style::bar("magenta"));
                bar.set_prefix("embedding files");
                bar.set_length(len);
                bar.enable_steady_tick(Duration::from_millis(120));
            }
        }
    };

    if cli.answer {
        let view =
            semantic_search_core::index_and_answer(&store, &query, &config, on_upload, on_poll)
                .await?;
        if let Some(bar) = &bar {
            bar.finish_and_clear();
        }
        println!("{}", view.answer);
        if !view.sources.is_empty() {
            println!();
            for (index, hit) in view.sources.iter().enumerate() {
                println!("{index}: {}", render(hit, cli.content, &palette, &root, theme));
            }
        }
    } else {
        let hits =
            semantic_search_core::index_and_search(&store, &query, &config, on_upload, on_poll)
                .await?;
        if let Some(bar) = &bar {
            bar.finish_and_clear();
        }
        for hit in &hits {
            println!("{}", render(hit, cli.content, &palette, &root, theme));
        }
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

fn render(
    hit: &DisplayHit,
    show_content: bool,
    palette: &Palette,
    root: &Path,
    theme: code_highlight::Theme,
) -> String {
    let prefix = if hit.is_web { "" } else { "./" };
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
    // parse context and code-highlight renders its own line-number gutter. Falls
    // through to a plain gutter over the chunk text if the file is unreadable
    // (e.g. a web hit or a file changed since indexing).
    if !hit.is_web
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

    use semantic_search_core::DisplayHit;

    use super::{Palette, render_snippet};

    /// A web hit so the snippet path renders the chunk text without reading a
    /// real file, isolating the gutter-vs-`cat -n` decision from the filesystem.
    fn hit(text: &str) -> DisplayHit {
        DisplayHit {
            label: "web://example".to_owned(),
            start_line: Some(4),
            num_lines: Some(2),
            score: 0.5,
            text: text.to_owned(),
            is_web: true,
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
