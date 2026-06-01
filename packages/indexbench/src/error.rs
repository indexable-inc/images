//! The single error type for the framework.
//!
//! Every fallible step in the library — deriving a machine id, spawning a macro
//! command, reading or appending to a history store, parsing a custom-metric
//! line — maps into one [`Error`] variant. The CLI converts this into a process
//! exit code in `main`, so the library never panics on operational failure.

use std::path::PathBuf;

use snafu::Snafu;

/// A failure raised while running or recording benchmarks.
///
/// Each variant names the resource it concerns (a path, a command, a metric
/// line) so an operator can act on the failure without reading a backtrace.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// The local hostname could not be read while deriving the machine id.
    #[snafu(display("failed to read hostname for machine id"))]
    Hostname {
        /// Underlying error from `nix::unistd::gethostname`.
        source: nix::Error,
    },

    /// A macro command failed to spawn.
    #[snafu(display("failed to spawn `{command}`"))]
    Spawn {
        /// The program that could not be launched.
        command: String,
        /// Underlying error from `fork`/`exec`.
        source: std::io::Error,
    },

    /// A macro command exited with a non-zero status or was killed by a signal.
    #[snafu(display("`{command}` exited unsuccessfully: {detail}"))]
    CommandFailed {
        /// The program that exited unsuccessfully.
        command: String,
        /// Human-readable exit detail (code or signal).
        detail: String,
    },

    /// A history-store file could not be read.
    #[snafu(display("failed to read history store at {}", path.display()))]
    StoreRead {
        /// The store file or directory that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A history-store file could not be written.
    #[snafu(display("failed to write history store at {}", path.display()))]
    StoreWrite {
        /// The store file or directory that could not be written.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A JSONL record in the history store did not parse.
    #[snafu(display("failed to parse history record in {}", path.display()))]
    StoreParse {
        /// The store file whose contents did not parse.
        path: PathBuf,
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// A `git` invocation backing the git history store failed.
    #[snafu(display("git {operation} failed: {detail}"))]
    Git {
        /// The git operation that failed (e.g. `mktree`, `commit-tree`).
        operation: String,
        /// Captured stderr or a description of the failure.
        detail: String,
    },

    /// A run's metrics could not be serialized to JSON.
    #[snafu(display("failed to serialize run to JSON"))]
    Serialize {
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// The CLI arguments were inconsistent in a way clap cannot express on its
    /// own (e.g. a mismatched count of `--cmd` and `--cmd-name`, or an `assert`
    /// with no budgets).
    #[snafu(display("{detail}"))]
    Usage {
        /// What was wrong and how to fix it.
        detail: String,
    },
}

/// The crate-wide result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
