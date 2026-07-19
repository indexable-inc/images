//! Error taxonomy. `Err` means the shell itself could not carry the run
//! forward; a command merely exiting nonzero is data in the [`Report`],
//! never an error.
//!
//! [`Report`]: crate::Report

use plumb_syntax::ParseError;
use snafu::Snafu;

/// Why an eval could not complete.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum Error {
    /// The source string is not in the plumb subset.
    #[snafu(display("{source}"), context(false))]
    Parse {
        /// The parser's diagnosis.
        source: ParseError,
    },

    /// Expansion referenced a variable that is not set (strict mode: bash
    /// would silently expand to nothing, which is how `rm -rf /$TYPO`
    /// happens).
    #[snafu(display("unset variable `${name}`"))]
    UnsetVar {
        /// The variable name.
        name: String,
    },

    /// A glob pattern matched nothing (strict, like bash `failglob`).
    #[snafu(display("glob matched nothing: `{pattern}`"))]
    GlobNoMatch {
        /// The pattern after expansion.
        pattern: String,
    },

    /// A redirection target could not be opened.
    #[snafu(display("cannot open `{path}` for redirection: {source}"))]
    Redirect {
        /// The file path.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// Plumbing failed (pipe creation, spawn machinery); not a command
    /// exiting nonzero.
    #[snafu(display("i/o error: {source}"))]
    Io {
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// A state-mutating builtin (`cd`, `export`, ...) was used as a pipeline
    /// stage, where bash would run it in a subshell and silently discard its
    /// effect.
    #[snafu(display("builtin `{name}` cannot be a pipeline stage"))]
    BuiltinInPipeline {
        /// The builtin name.
        name: String,
    },

    /// A builtin was misused; the message says how.
    #[snafu(display("{name}: {message}"))]
    BuiltinUsage {
        /// The builtin name.
        name: String,
        /// What was wrong.
        message: String,
    },

    /// An indexed run reference (dollar-brace `o[N]` form) could not
    /// resolve.
    #[snafu(display("run reference: {message}"))]
    RunRef {
        /// What failed to resolve.
        message: String,
    },

    /// A `$(...)` substitution produced more output than the configured
    /// budget; its value would have been silently wrong.
    #[snafu(display("command substitution output exceeded {limit} bytes"))]
    SubstitutionOverflow {
        /// The configured substitution budget.
        limit: usize,
    },

    /// The `exit` builtin ran: the embedder should wind down with `code`.
    #[snafu(display("exit {code}"))]
    ExitRequested {
        /// Requested exit code.
        code: i32,
    },
}
