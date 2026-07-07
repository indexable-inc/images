//! Python async helpers the generated glue calls.
//!
//! Everything here is a thin layer over `pyo3-async-runtimes`, so generated
//! code only ever names `unibind_runtime`.

use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt as _;

use pyo3::{Bound, PyAny, PyResult, Python};

use crate::shared::panic_text;
// Generated glue names `unibind_runtime::py::SharedStream`; the definition
// moved to the shared backend module when the JVM backend started using it.
pub use crate::shared::SharedStream;

/// Convert a Rust future into an awaitable Python object.
///
/// Single indirection point so generated glue only names `unibind-runtime`;
/// the asyncio-cancel-drops-the-future guarantee is inherited from
/// `pyo3-async-runtimes`.
///
/// The unwind is caught here because `pyo3-async-runtimes` reports a
/// panicking future as `RustPanic: rust future panicked: unknown error`,
/// discarding the payload; re-raising `pyo3::panic::PanicException` with
/// the panic text matches the sync boundary, where pyo3 itself raises
/// `PanicException` carrying the message.
///
/// # Errors
///
/// Fails when no asyncio event loop can be resolved for the calling
/// context.
pub fn future_into_py<'py, F, T>(py: Python<'py>, fut: F) -> PyResult<Bound<'py, PyAny>>
where
    F: Future<Output = PyResult<T>> + Send + 'static,
    T: for<'a> pyo3::IntoPyObject<'a> + Send + 'static,
{
    let caught = AssertUnwindSafe(fut).catch_unwind();
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        match caught.await {
            Ok(result) => result,
            Err(payload) => Err(pyo3::panic::PanicException::new_err(panic_text(
                payload.as_ref(),
            ))),
        }
    })
}
