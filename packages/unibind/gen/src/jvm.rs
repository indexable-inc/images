//! Emit the JVM host files for one interface.
//!
//! The Java Panama binding and the Kotlin sugar come straight from
//! `unibind-backend-jvm` -- the same generators that share a layout model
//! with the macro-side `extern "C"` glue -- so this module only adapts them
//! onto the [`HostEmitter`] seam.

use unibind_core::ir::Interface;

use crate::host::{EmitError, HostEmitter, HostFile};

/// The JVM emitter; writes `unibind/<module>/*.java` plus `<Module>.kt`
/// under the output root.
pub struct JvmEmitter;

impl HostEmitter for JvmEmitter {
    fn target(&self) -> &'static str {
        "jvm"
    }

    fn emit(&self, interface: &Interface) -> Result<Vec<HostFile>, EmitError> {
        let mut sources = unibind_backend_jvm::generate_java(interface)
            .map_err(|error| EmitError {
                message: error.message,
            })?;
        sources.extend(
            unibind_backend_jvm::generate_kotlin(interface).map_err(|error| EmitError {
                message: error.message,
            })?,
        );
        Ok(sources
            .into_iter()
            .map(|file| HostFile {
                path: file.path,
                contents: file.content,
            })
            .collect())
    }
}
