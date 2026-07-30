//! Typed errors for every fallible operation on a `.memories` corpus. The CLI
//! edge in `main.rs` is the only place these become `anyhow` reports.

use snafu::Snafu;
use std::path::PathBuf;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Failed to read {path}: {source}", path = path.display()))]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to write {path}: {source}", path = path.display()))]
    WriteFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to list {path}: {source}", path = path.display()))]
    ReadDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to create {path}: {source}", path = path.display()))]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display(
        "--dir {path} has no `.memories` directory to read",
        path = path.display(),
    ))]
    MissingMemoriesDir { path: PathBuf },

    #[snafu(display(
        "No home directory, so the default `~/.memories` root cannot be resolved; \
         pass --dir to name the directories explicitly"
    ))]
    NoHomeDir,

    #[snafu(display("Failed to read the current directory: {source}"))]
    NoCurrentDir { source: std::io::Error },

    #[snafu(display("No memory matches the slug {slug}"))]
    UnknownSlug { slug: String },

    #[snafu(display(
        "{slug} already exists at {path}; edit it, or validate it, or pick another slug",
        path = path.display(),
    ))]
    SlugExists { slug: String, path: PathBuf },

    #[snafu(display("{path}: {rule}: {message}", path = path.display()))]
    Malformed {
        path: PathBuf,
        rule: &'static str,
        message: String,
    },

    #[snafu(display(
        "Refusing to rewrite {path}: its frontmatter no longer parses ({message})",
        path = path.display(),
    ))]
    UnwritableFrontmatter { path: PathBuf, message: String },

    #[snafu(display(
        "--based-on {path} matches nothing under {root}",
        root = root.display(),
    ))]
    BasedOnMissing { path: String, root: PathBuf },

    #[snafu(display("BM25 search failed: {source}"))]
    Search { source: file_search::Error },

    #[snafu(display("based_on entry {pattern} is not a valid glob: {source}"))]
    BadGlob {
        pattern: String,
        source: glob::PatternError,
    },

    #[snafu(display("Failed to expand the glob {pattern}: {source}"))]
    GlobWalk {
        pattern: String,
        source: glob::GlobError,
    },

    #[snafu(display("Path {path} is not valid UTF-8, so it cannot be matched as a glob", path = path.display()))]
    NonUtf8Path { path: PathBuf },

    #[snafu(display("Failed to render JSON: {source}"))]
    Json { source: serde_json::Error },

    #[snafu(display("Failed to read the body from stdin: {source}"))]
    ReadStdin { source: std::io::Error },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
