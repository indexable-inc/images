//! Language-agnostic interface representation for unibind.
//!
//! `#[unibind::export]` hands the annotated module here: [`lower_module`]
//! turns the syn items into an [`ir::Interface`], and [`embed`] renders the
//! serialized interface into a link-section constant so later phases can
//! read the contract straight from a built artifact. Backends such as
//! `unibind-backend-py` consume the IR and render the language-specific
//! binding code.

pub mod embed;
pub mod ir;
mod lower;

pub use lower::{export_backends, lower_module, strip_unibind_attrs, Backend, LowerError};
