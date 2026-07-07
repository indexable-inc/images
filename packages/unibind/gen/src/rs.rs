//! The Rust client-crate emitter: the `rs` target of the [`HostEmitter`]
//! seam. All rendering lives in `unibind-backend-rs` (the same library the
//! `#[unibind::export]` macro's `rs` feature uses for the engine half), so
//! this adapter only maps the seam's types.

use unibind_core::ir::Interface;

use crate::host::{EmitError, HostEmitter, HostFile};

/// Emit a complete `<name>-client` crate wrapping one engine cdylib: a
/// Cargo.toml, the safe `Engine` wrapper sources, and (in workspace mode)
/// the package.nix registry marker.
pub struct RsEmitter {
    /// The generated crate's `[package] name`.
    pub crate_name: String,
    /// Resolve dependencies through `workspace = true` (and emit the
    /// package.nix marker) instead of concrete versions.
    pub workspace_deps: bool,
}

impl HostEmitter for RsEmitter {
    fn target(&self) -> &'static str {
        "rs"
    }

    fn emit(&self, interface: &Interface) -> Result<Vec<HostFile>, EmitError> {
        let rendered = unibind_backend_rs::render_client(
            interface,
            &unibind_backend_rs::ClientOptions {
                crate_name: self.crate_name.clone(),
                workspace_deps: self.workspace_deps,
            },
        )
        .map_err(|error| EmitError {
            message: error.message,
        })?;
        Ok(rendered
            .files
            .into_iter()
            .map(|file| HostFile {
                path: file.path,
                contents: file.contents,
            })
            .collect())
    }
}
