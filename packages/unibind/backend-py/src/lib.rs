//! Render a lowered [`unibind_core::ir::Interface`] into `pyo3` binding code.
//!
//! The backend targets the incumbent binding library rather than raw FFI: it
//! emits `#[pyo3::pyfunction]` wrappers around the user's functions, attaches
//! `#[pyo3::pyclass]` to record structs, builds the exception hierarchy with
//! `pyo3::create_exception!`, renders each unit enum as an `enum.StrEnum`
//! built at module init, wraps objects and streams in `#[pyo3::pyclass]`
//! handles, and registers everything in one imperative `#[pyo3::pymodule]`.
//! The consuming crate therefore depends on `pyo3` directly (with
//! `extension-module` for a wheel-shaped cdylib), and the generated code
//! compiles against `pyo3` 0.28 with `abi3-py311`. Glue for async, stream,
//! and object exports also calls `unibind_py_runtime`, so those consumers
//! add `unibind-runtime` and `unibind-py-runtime`.

mod ctx;
mod error;
mod function;
mod module;
mod object;
mod record;
mod resource;
mod sig;
pub mod stream;
mod unit_enum;

pub use module::render;
