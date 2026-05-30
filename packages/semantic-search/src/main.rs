//! `semantic-search`: a daemon-free, content-addressed semantic code search.
//!
//! Every run rebuilds a local manifest (cheap; unchanged files are not
//! re-hashed), uploads only content the store is missing, waits for it to
//! embed, then searches. No daemon, no `--sync` flag: new files are picked up
//! and embedded automatically at search time. `--no-sync` skips that for a pure
//! offline search. Results are scoped to the current checkout via the manifest.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;
use semantic_search::{
    Config, DEFAULT_STORE, Db, DisplayHit, Manifest, MixedbreadStore, SearchOptions,
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

    let config = Config::default();
    let mut db = Db::open()?;
    let previous = db.load(&root)?;
    let manifest = Manifest::build(&root, Some(&previous), config.max_file_bytes)?;
    db.save(&root, &manifest)?;

    let store_name = cli.store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base_url = cli
        .base_url
        .unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let store = MixedbreadStore::from_login(base_url).await?;

    if !cli.no_sync {
        let report =
            semantic_search::sync(&store, &store_name, &root, &manifest, config.max_files).await?;
        if report.uploaded > 0 {
            eprintln!("indexing {} new file(s)...", report.uploaded);
            let indexed =
                semantic_search::wait_until_indexed(&store, &store_name, INDEX_TIMEOUT).await?;
            if !indexed {
                eprintln!(
                    "warning: indexing still in progress after {}s; results may be incomplete",
                    INDEX_TIMEOUT.as_secs()
                );
            }
        }
    }

    let options = SearchOptions {
        rerank: !cli.no_rerank,
        agentic: cli.agentic,
    };
    let top_k = cli.max_count.max(1);

    if cli.answer {
        let view = semantic_search::ask(
            &store,
            &store_name,
            &manifest,
            &cli.pattern,
            top_k,
            options,
            cli.web,
        )
        .await?;
        println!("{}", view.answer);
        if !view.sources.is_empty() {
            println!();
            for (index, hit) in view.sources.iter().enumerate() {
                println!("{index}: {}", render(hit, cli.content));
            }
        }
    } else {
        let hits = semantic_search::search(
            &store,
            &store_name,
            &manifest,
            &cli.pattern,
            top_k,
            options,
            cli.web,
        )
        .await?;
        for hit in &hits {
            println!("{}", render(hit, cli.content));
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

fn render(hit: &DisplayHit, show_content: bool) -> String {
    let location = match (hit.start_line, hit.num_lines) {
        (Some(start), Some(num)) => format!(":{}-{}", start + 1, start + 1 + num),
        (Some(start), None) => format!(":{}", start + 1),
        _ => String::new(),
    };
    let prefix = if hit.is_web { "" } else { "./" };
    let percent = hit.score * 100.0;
    let head = format!("{prefix}{}{location} ({percent:.2}% match)", hit.label);
    if show_content && !hit.text.is_empty() {
        format!("{head}\n{}", hit.text)
    } else {
        head
    }
}
