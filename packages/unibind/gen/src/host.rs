//! The host-language emission seam.
//!
//! One [`HostEmitter`] per target language turns a lowered
//! [`Interface`] into plain files. This trait is the contract the later
//! phases implement: the TypeScript `.d.ts` emitter (phase 3, issue #1993)
//! and the Elixir `.ex`/`@spec` emitter (phase 5, issue #1995) plug in here,
//! next to [`crate::py::PyEmitter`]. The seam stays language-agnostic on
//! purpose: per-target options (package names, wrapper skipping) live on the
//! concrete emitter struct, never on the trait.

use std::fmt;
use std::path::Path;

use anyhow::Context as _;
use unibind_core::ir::Interface;

/// One generated host-language file, path relative to the output root.
pub struct HostFile {
    pub path: String,
    pub contents: String,
}

/// Render an interface into host-language files for one target.
pub trait HostEmitter {
    /// Human-readable target name for diagnostics ("py", "ts", "ex").
    fn target(&self) -> &'static str;

    /// Render every file for `interface`.
    ///
    /// # Errors
    ///
    /// Fails for interface surface the target cannot express.
    fn emit(&self, interface: &Interface) -> Result<Vec<HostFile>, EmitError>;
}

/// An emission failure: what the target could not express and what to do.
#[derive(Debug)]
pub struct EmitError {
    pub message: String,
}

impl fmt::Display for EmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for EmitError {}

/// Write emitted files under `out_dir`, creating parent directories.
///
/// # Errors
///
/// Fails when a directory or file cannot be created or written.
pub fn write_host_files(out_dir: &Path, files: &[HostFile]) -> anyhow::Result<()> {
    for file in files {
        let path = out_dir.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, &file.contents)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}
