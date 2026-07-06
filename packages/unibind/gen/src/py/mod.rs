//! Emit the Python host files for one interface.
//!
//! Three files land under `<package>/`: the `<module>.pyi` stub typing the
//! extension module, the `py.typed` marker that makes type checkers read it,
//! and (unless skipped) the wrapper `__init__.py` re-exporting the module's
//! public names.

mod stub;
mod types;
mod wrapper;

use unibind_core::ir::{Asyncness, Interface};

use crate::host::{EmitError, HostEmitter, HostFile};

/// The Python emitter; writes into `<package>/` under the output root.
pub struct PyEmitter {
    /// Import-package name the files land under (e.g. `scipql`).
    pub package: String,
    /// Skip the wrapper `__init__.py`, for packages that keep a hand-written
    /// one next to the generated stub.
    pub skip_init: bool,
}

impl HostEmitter for PyEmitter {
    fn target(&self) -> &'static str {
        "py"
    }

    fn emit(&self, interface: &Interface) -> Result<Vec<HostFile>, EmitError> {
        reject_unrendered_surface(interface)?;

        // Same module-name rule the pyo3 backend applies when it registers
        // the `#[pymodule]`.
        let module_name = interface
            .names
            .py
            .clone()
            .unwrap_or_else(|| interface.name.clone());
        let mut files = vec![
            HostFile {
                path: format!("{}/{module_name}.pyi", self.package),
                contents: stub::render(interface),
            },
            HostFile {
                path: format!("{}/py.typed", self.package),
                contents: String::new(),
            },
        ];
        if !self.skip_init {
            files.push(HostFile {
                path: format!("{}/__init__.py", self.package),
                contents: wrapper::render(interface, &module_name),
            });
        }
        Ok(files)
    }
}

/// Refuse the IR surface the phase 1 stub emitter does not render, with the
/// same pointers the pyo3 backend gives.
fn reject_unrendered_surface(interface: &Interface) -> Result<(), EmitError> {
    if let Some(object) = interface.objects.first() {
        return Err(EmitError {
            message: format!(
                "`{}` is a #[unibind::object]; objects land in phase 2 (issue #1992)",
                object.name
            ),
        });
    }
    if let Some(data_enum) = interface.enums.first() {
        return Err(EmitError {
            message: format!("`{}` is a data enum, which phase 1 does not render", data_enum.name),
        });
    }
    if let Some(function) = interface
        .functions
        .iter()
        .find(|function| matches!(function.asyncness, Asyncness::Async))
    {
        return Err(EmitError {
            message: format!(
                "`{}` is async; async functions land in phase 2 (issue #1992)",
                function.name
            ),
        });
    }
    Ok(())
}
