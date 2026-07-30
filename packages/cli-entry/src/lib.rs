//! One implementation of the `fn main` every clap-based binary here needs.
//!
//! Four binaries carried byte-identical copies of parse-dispatch-report
//! (`memories`, `indexbench`, `file-search`, `skill-lint`). That is real
//! duplication by this repo's own standard, and it is the kind a similarity
//! gate cannot let a fifth binary add: `clone.toml` sets `diff_pct = 0.0`, so
//! the next new CLI fails the gate on the one block in a Rust program whose
//! shape is not the author's to choose. Rewriting each `main` differently to
//! defeat the metric would make every one of them worse, so the pattern is
//! removed instead of exempted (ENG-11468).

use std::fmt::Display;
use std::process::ExitCode;

use clap::Parser;

/// Parse `C`, hand it to `body`, and turn an error into `name: <error>` on
/// stderr plus a failure exit.
///
/// `body` returns the `ExitCode` rather than a unit, because a lint or a scan
/// distinguishes "ran, and the answer is no" from "could not run": the first is
/// a chosen non-zero code, the second is this function's `FAILURE`. Collapsing
/// them would make a found-a-defect exit indistinguishable from a crash.
///
/// The error is formatted with `{:#}`, which on an `anyhow::Error` prints the
/// whole cause chain rather than only the outermost message. A bare `{}` drops
/// the context every `.context(...)` call was added to supply.
pub fn run<C, E>(name: &str, body: impl FnOnce(C) -> Result<ExitCode, E>) -> ExitCode
where
    C: Parser,
    E: Display,
{
    match body(C::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{name}: {error:#}");
            ExitCode::FAILURE
        }
    }
}
