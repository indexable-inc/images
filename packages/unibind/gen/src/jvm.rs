//! Emit the Java host file for one interface.
//!
//! One file lands: `<Class>.java`, the single `final class` wrapping the
//! C-ABI symbols through the FFM API. The rendering itself lives in
//! `unibind-backend-jvm` (next to the glue renderer, so symbol names and
//! wire layouts cannot drift apart); this emitter only decides where the
//! class lands, mapping the package's dots onto directories.

use unibind_core::ir::Interface;

use crate::host::{EmitError, HostEmitter, HostFile};

/// The Java emitter; writes the class at its package's directory path.
pub struct JvmEmitter {
    /// Java package the class is declared in (`com.example.sample`); the
    /// file lands under the matching directory tree. `None` puts the class
    /// in the unnamed package at the output root.
    pub package: Option<String>,
}

impl HostEmitter for JvmEmitter {
    fn target(&self) -> &'static str {
        "jvm"
    }

    fn emit(&self, interface: &Interface) -> Result<Vec<HostFile>, EmitError> {
        let host = unibind_backend_jvm::host_class(interface, self.package.as_deref()).map_err(
            |error| EmitError {
                message: error.message,
            },
        )?;
        let path = match &self.package {
            Some(package) => format!("{}/{}", package.replace('.', "/"), host.file_name),
            None => host.file_name,
        };
        Ok(vec![HostFile {
            path,
            contents: host.source,
        }])
    }
}
