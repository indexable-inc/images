//! Render a lowered [`unibind_core::ir::Interface`] into C-ABI shims and
//! the Java class that calls them through the FFM API.
//!
//! Unlike the other backends, the jvm backend targets no binding library:
//! every exported function becomes one `extern "C"` symbol with the uniform
//! shape `fn(args: *const u8, len: usize, out: *mut RawBuf)`, and values
//! cross in `unibind-jvm-runtime`'s length-prefixed wire format. The
//! matching Java side is a single generated class (records, exception
//! hierarchy, wire codec, FFM plumbing) from [`host_class`], which
//! `unibind-gen`'s `JvmEmitter` writes to disk. The consuming crate builds
//! a `cdylib` and depends on `unibind-jvm-runtime` directly; the JVM needs
//! `--enable-native-access` for the generated lookups.

mod error;
mod function;
mod host;
mod module;
mod names;
mod record;
mod ty;

pub use host::{HostClass, host_class};
pub use module::render;

/// The rendered output for one interface.
pub struct RenderedInterface {
    /// Sibling items for the exported module: the hidden glue module with
    /// the record codecs, error mappers, `extern "C"` shims, and the
    /// buffer-free symbol.
    pub glue: proc_macro2::TokenStream,
    /// Attributes to attach to each record struct, index-aligned with the
    /// interface's records. The jvm backend reads records with plain field
    /// access, so every entry is empty.
    pub records: Vec<RenderedRecord>,
}

/// Attributes for one record struct; empty for this backend.
pub struct RenderedRecord {
    /// Outer attributes for the struct itself.
    pub outer: Vec<syn::Attribute>,
    /// Attributes for each field, index-aligned with the record's fields.
    pub fields: Vec<Vec<syn::Attribute>>,
}

/// A rendering failure; the macro positions it at the exported module.
#[derive(Debug)]
pub struct RenderError {
    /// What went wrong and what to do instead.
    pub message: String,
}

impl RenderError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
