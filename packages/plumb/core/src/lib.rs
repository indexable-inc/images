//! libplumb: an embeddable shell where every run is a value.
//!
//! [`Shell::eval`] runs a strict bash subset (pipes, redirections, quoting,
//! `$VAR`, globs, `$(...)`, `&&`/`||`/`;`, `&`) and returns a [`Report`]:
//! per-stage argv, status, timing, and bounded captures of every stream
//! that flowed through the pipeline, including what each pipe stage fed the
//! next. Every run stays addressable afterwards: `${o[N]}` / `${e[N]}` /
//! `${s[N]}` resolve run N's final stdout / stderr / status, `${o[N][K]}`
//! resolves pipe stage K, negative indexes count from the latest, and
//! `$o` / `$e` / `$s` alias the most recent run. Any earlier stream,
//! including intermediate pipe data, can feed later commands without
//! re-running anything.
//!
//! ```no_run
//! let shell = plumb_core::Shell::new(plumb_core::Config::default())?;
//! let report = shell.eval("cargo clippy 2>&1 | tail -n 5")?;
//! assert_eq!(report.pipelines[0].stages.len(), 2);
//! // Everything tail consumed, reused without re-running clippy:
//! let hits = shell.eval("echo ${o[1][0]} | rg dead")?;
//! # let _ = hits;
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
