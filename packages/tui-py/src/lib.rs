//! Python bindings for the `tui` PTY-backed terminal management library.
//!
//! All blocking calls release the GIL via `Python::detach`, so multiple
//! Python threads can drive the same manager concurrently.

#![allow(
    clippy::missing_const_for_fn,
    reason = "pyo3 getter methods cannot be const because they are dispatched through the pymethod vtable"
)]
#![allow(
    clippy::struct_excessive_bools,
    reason = "VT100 cell attributes are intrinsically four parallel booleans"
)]

mod manager;
mod types;

use pyo3::prelude::*;

#[pymodule]
fn _superglide_tui(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<manager::TuiManager>()?;
    module.add_class::<manager::TuiInstance>()?;
    module.add_class::<types::FullOutput>()?;
    module.add_class::<types::StyledCell>()?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
