//! The rendering context threaded through every renderer.

use proc_macro2::Ident;
use quote::format_ident;
use unibind_core::ir;

/// What renderers need besides the item at hand: the exported module's
/// identifier (user items resolve through `super::<user>::`) and the whole
/// interface for name resolution.
pub struct Ctx<'a> {
    pub user: &'a Ident,
    pub interface: &'a ir::Interface,
}

impl Ctx<'_> {
    /// Whether a [`ir::Type::Named`] names an object rather than a record;
    /// the IR spells both the same way, so the interface's object list
    /// decides which wrapper the value crosses through.
    pub fn is_object(&self, name: &str) -> bool {
        self.interface
            .objects
            .iter()
            .any(|object| object.name == name)
    }
}

/// The glue-side `#[pyclass]` wrapper for object `name`.
pub fn object_wrapper_ident(name: &str) -> Ident {
    format_ident!("UnibindObject{name}")
}
