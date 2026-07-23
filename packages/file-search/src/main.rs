use clap::{Parser, Subcommand};
use file_search::{Result, SearchIndex, SearchIndexReader};
use std::path::PathBuf;
use std::process::ExitCode;

/// BM25 file indexer and searcher built on Tantivy.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Directory where the Tantivy index is stored.
    #[arg(long, value_name = "PATH", global = true)]
    index_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Walk a directory and add every text file to the index.
    Index {
        /// Directory to walk.
        directory: PathBuf,

        /// Walk hidden files and ignore `.gitignore` entries.
        #[arg(long)]
        no_gitignore: bool,
    },

    /// Search the index for files matching the query.
    Search {
        /// Query string in Tantivy query syntax.
        query: String,

        /// Maximum number of results.
        #[arg(long, default_value_t = 10)]
        limit: usize,

        /// Restrict matches to files under this directory.
        #[arg(long, value_name = "PATH")]
        filter: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("file-search: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let index_dir = cli.index_dir.unwrap_or_else(default_index_dir);

    match cli.command {
        Command::Index {
            directory,
            no_gitignore,
        } => {
            let mut index = SearchIndex::open_or_create(&index_dir)?;
            let stats = index.index_directory(&directory, !no_gitignore)?;
            println!(
                "indexed {} file(s), skipped {} ({} error{}) into {}",
                stats.files_indexed,
                stats.files_skipped,
                stats.errors.len(),
                if stats.errors.len() == 1 { "" } else { "s" },
                index_dir.display(),
            );
            for (path, err) in stats.errors {
                eprintln!("  skip {}: {err}", path.display());
            }
        }
        Command::Search {
            query,
            limit,
            filter,
        } => {
            // Search avoids opening the writer so it can run against a
            // shared or read-only index, and so it doesn't conflict with a
            // concurrent `index` invocation.
            let reader = SearchIndexReader::open(&index_dir)?;
            let results = reader.search(&query, limit, filter.as_deref())?;
            if results.is_empty() {
                println!("no results");
            }
            for hit in results {
                println!(
                    "{:>7.3}  {}{}",
                    hit.score,
                    hit.path,
                    chunk_label(hit.chunk_offset),
                );
            }
        }
    }

    Ok(())
}

fn chunk_label(offset: u64) -> String {
    if offset == 0 {
        String::new()
    } else {
        format!("  @{offset}")
    }
}

fn default_index_dir() -> PathBuf {
    std::env::var_os("FILE_SEARCH_INDEX_DIR").map_or_else(
        || {
            let base = dirs_cache_dir().unwrap_or_else(std::env::temp_dir);
            base.join("file-search").join("index")
        },
        PathBuf::from,
    )
}

fn dirs_cache_dir() -> Option<PathBuf> {
    // Avoid pulling in the `dirs` crate for one path. Honor XDG on Unix and
    // fall back to ~/.cache; on macOS we follow XDG too so the path stays
    // predictable across systems.
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(xdg));
    }
    let home = std::env::var_os("HOME").filter(|s| !s.is_empty())?;
    Some(PathBuf::from(home).join(".cache"))
}
