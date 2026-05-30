//! Python bindings for the `tui` PTY-backed terminal management library.
//!
//! All blocking calls release the GIL via `Python::detach`, so multiple
//! Python threads can drive the same manager concurrently. Async methods
//! return native asyncio-awaitable coroutines bridged through
//! pyo3-async-runtimes.

#![allow(
    clippy::missing_const_for_fn,
    reason = "pyo3 getter methods cannot be const because they are dispatched through the pymethod vtable"
)]

mod dashboard;
mod manager;
mod publish;
mod types;

use pyo3::prelude::*;

#[pymodule]
fn _tui(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<manager::TuiInstance>()?;
    module.add_class::<types::StyledCell>()?;
    module.add_class::<dashboard::Dashboard>()?;
    module.add_class::<publish::Publisher>()?;
    module.add_function(wrap_pyfunction!(dashboard::serve, module)?)?;
    module.add_function(wrap_pyfunction!(publish::publish, module)?)?;
    module.add_function(wrap_pyfunction!(publish::ensure_published, module)?)?;
    module.add_function(wrap_pyfunction!(publish::socket_dir, module)?)?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
