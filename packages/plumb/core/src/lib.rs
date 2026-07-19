//! libplumb: an embeddable shell where every run is a value.
//!
//! [`Shell::eval`] runs a strict bash subset (pipes, redirections, quoting,
//! `$VAR`, globs, `$(...)`, `&&`/`||`/`;`, `&`) and returns a [`Report`]:
//! per-stage argv, status, timing, and bounded captures of every stream
//! that flowed through the pipeline, including what each pipe stage fed the
//! next. Outputs auto-bind to shell variables (`$oN`, `$eN`, `$sN`,
//! `$oN_K`, `$eN_K`, and `$o`/`$e`/`$s` for the last run), so any earlier
//! run's data, including intermediate pipe data, can feed later commands
//! without re-running anything.
//!
//! ```no_run
//! let shell = plumb_core::Shell::new(plumb_core::Config::default())?;
//! let report = shell.eval("cargo clippy 2>&1 | tail -n 5")?;
//! assert_eq!(report.pipelines[0].stages.len(), 2);
//! let full_clippy = shell.var("o1_0"); // everything tail consumed
//! # let _ = full_clippy;
//! # Ok::<(), plumb_core::Error>(())
//! ```
//!
//! [`Shell`] is a cheap cloneable handle over shared state: concurrent
//! evals (threads, `&` background items) see each other's variables, cwd,
//! and run history.

mod engine;
mod error;
mod report;
mod shell;
#[cfg(test)]
mod tests;

pub use error::Error;
pub use plumb_syntax as syntax;
pub use report::{Capture, PipelineRun, Report, Stage};
pub use shell::{Config, RunHandle, Shell};
