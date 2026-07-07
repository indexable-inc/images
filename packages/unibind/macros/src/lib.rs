//! Write-once binding attributes.
//!
//! `#[unibind::export]` on an inline module lowers every `pub fn` in it,
//! together with the `#[unibind::record]` and `#[unibind::error]` items,
//! into one language-agnostic interface (see `unibind-core`), embeds the
//! serialized interface in the built artifact, and renders binding code for
//! every backend enabled by cargo features (`py` renders `pyo3`, `ts`
//! renders `napi-rs`).
//!
//! ```ignore
//! #[unibind::export]
//! mod _mylib {
//!     /// Rows come back as native classes.
//!     #[unibind::record]
//!     #[derive(Clone)]
//!     pub struct Row {
//!         pub id: u64,
//!         pub name: String,
//!     }
//!
//!     /// Everything the boundary raises.
//!     #[unibind::error(py(base = "ValueError"))]
//!     pub enum MyError {
//!         /// The store is gone.
//!         StoreGone { message: String },
//!     }
//!
//!     /// Doc comments become docstrings.
//!     pub fn rows(store: &str, #[unibind(default = 10)] limit: usize) -> Result<Vec<Row>, MyError> {
//!         let _ = (store, limit);
//!         Ok(Vec::new())
//!     }
//! }
//! ```

mod expand;

use proc_macro::TokenStream;

/// Export an inline module as the crate's binding boundary.
///
/// Every `pub fn` in the module is exported; private items pass through
/// untouched. The attribute accepts `py(name = "...")` to rename the Python
/// module (it defaults to the Rust module name, which also names the
/// `PyInit_` symbol of the built extension), and `backends(py, ts)` to pin
/// which backends render glue. Without `backends(...)` every
/// feature-enabled backend renders; a whole-workspace cargo build unifies
/// features across every unibind consumer, so a crate in a workspace that
/// mixes backend features names its own backends explicitly (the ones
/// whose runtime dependencies it declares).
///
/// Glue for async functions, `UniStream` returns, and `#[unibind::object]`
/// types calls into `unibind_runtime::py`, so a crate exporting any of
/// those adds `unibind-runtime` with the `py` feature to its dependencies.
/// The ts backend's consumers depend on `napi` (`napi6` + `tokio_rt`),
/// `napi-derive`, `tokio` (`sync` + `macros`), and `unibind-runtime`, and
/// build a cdylib with `napi_build::setup()`.
#[proc_macro_attribute]
pub fn export(args: TokenStream, item: TokenStream) -> TokenStream {
    expand::export(args.into(), item.into()).into()
}

/// Mark a plain-data struct inside a `#[unibind::export]` module.
///
/// The struct crosses the boundary by value: Python sees a native class
/// with one read-only attribute per field and a positional constructor.
/// Fields must be `pub` and owned; `#[unibind(py(name = "..."))]` renames a
/// field. The struct needs `Clone` for attribute access from Python.
#[proc_macro_attribute]
pub fn record(_args: TokenStream, item: TokenStream) -> TokenStream {
    expand::marker_outside_export(
        item.into(),
        "#[unibind::record] only takes effect inside a #[unibind::export] \
         module; declare the struct inside the exported module",
    )
    .into()
}

/// Mark an error enum inside a `#[unibind::export]` module.
///
/// The enum becomes an exception hierarchy: one base class named after the
/// enum and one subclass per variant, carrying the variant's `Display`
/// text. `py(base = "...")` picks the Python built-in the base class
/// extends (`Exception` by default); the enum must implement `Display`.
#[proc_macro_attribute]
pub fn error(_args: TokenStream, item: TokenStream) -> TokenStream {
    expand::marker_outside_export(
        item.into(),
        "#[unibind::error] only takes effect inside a #[unibind::export] \
         module; declare the enum inside the exported module",
    )
    .into()
}

/// Mark a stateful handle inside a `#[unibind::export]` module.
///
/// The struct crosses the boundary by reference: the target language holds
/// a wrapped handle whose state stays on the Rust side. Methods take
/// `&self` (use interior mutability for state), one associated function
/// marked `#[unibind(constructor)]` makes the object constructible, and
/// methods may be async or return streams. `object(resource)` requires a
/// `close` method and adds close()/async-with plus a warning when the
/// resource leaks unclosed. The wrapped struct crosses threads inside an
/// `Arc`, so it must be `Send + Sync`.
#[proc_macro_attribute]
pub fn object(_args: TokenStream, item: TokenStream) -> TokenStream {
    expand::marker_outside_export(
        item.into(),
        "#[unibind::object] only takes effect inside a #[unibind::export] \
         module; declare the struct inside the exported module",
    )
    .into()
}
