//! Produce a SCIP index by shelling out to `rust-analyzer scip`.

use std::path::Path;
use std::process::Command;

use snafu::{ResultExt as _, ensure};

use crate::error::{Error, RunRustAnalyzerSnafu};

/// Run `rust-analyzer scip <project> --output <output>`.
///
/// `rust-analyzer` (and the `cargo`/`rustc` it drives) must be on `PATH`; the
/// `scipql` CLI bakes them in. This loads the whole cargo workspace, so it is
/// the slow step; `output` is the protobuf SCIP index the rest of the pipeline
/// reads.
///
/// # Errors
///
/// Fails if `rust-analyzer` cannot be spawned or exits non-zero.
pub fn index(project: &Path, output: &Path) -> Result<(), Error> {
    // `SCIPQL_RUST_ANALYZER` lets a wrapper bake an absolute path; falls back to
    // `rust-analyzer` on PATH (the CLI bakes the repo's pinned toolchain).
    let rust_analyzer =
        std::env::var("SCIPQL_RUST_ANALYZER").unwrap_or_else(|_| "rust-analyzer".to_owned());
    let status = Command::new(rust_analyzer)
        .arg("scip")
        .arg(project)
        .arg("--output")
        .arg(output)
        .status()
        .context(RunRustAnalyzerSnafu)?;
    ensure!(
        status.success(),
        crate::error::RustAnalyzerFailedSnafu {
            code: status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
        }
    );
    Ok(())
}
