//! Write-once binding attributes.
//!
//! `#[unibind::export]` on an inline module lowers every `pub fn` in it,
//! together with the `#[unibind::record]` and `#[unibind::error]` items,
//! into one language-agnostic interface (see `unibind-core`), embeds the
//! serialized interface in the built artifact, and renders binding code for
//! every backend enabled by cargo features (`py` renders `pyo3`).
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
/// `PyInit_` symbol of the built extension).
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

/// Reserved for stateful handles; lands with resources in phase 2
/// (issue #1992).
#[proc_macro_attribute]
pub fn object(_args: TokenStream, item: TokenStream) -> TokenStream {
    expand::marker_outside_export(
        item.into(),
        "#[unibind::object] lands with resources in phase 2 (issue #1992); \
         phase 0 covers sync functions, records, and errors",
    )
    .into()
}
